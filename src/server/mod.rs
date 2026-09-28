//! A synchronous, single-workspace LSP server sharing the CLI analysis pipeline.

mod text;
mod transport;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::analysis::{self, Analysis};
use crate::config::Workspace;
use crate::diagnostics::{Applicability, Diagnostic};
use crate::workspace::{self, LoadOptions, LoadScope};
use text::{Change, Range, TextMap, apply_changes};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Lifecycle {
    New,
    Initializing,
    Running,
    Shutdown,
}

struct Buffer {
    uri: String,
    version: i32,
    text: String,
    synchronized: bool,
}

struct Server {
    root: PathBuf,
    options: LoadOptions,
    lifecycle: Lifecycle,
    buffers: BTreeMap<PathBuf, Buffer>,
    published: BTreeSet<String>,
    related_information: bool,
    version_support: bool,
    code_description: bool,
    quick_fixes: bool,
    watch_registration: bool,
}

#[derive(Deserialize)]
struct DocumentId {
    uri: String,
}

#[derive(Deserialize)]
struct VersionedDocument {
    uri: String,
    version: i32,
}

#[derive(Deserialize)]
struct OpenDocument {
    uri: String,
    version: i32,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenParams {
    text_document: OpenDocument,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentParams {
    text_document: DocumentId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeParams {
    text_document: VersionedDocument,
    content_changes: Vec<Change>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActionParams {
    text_document: DocumentId,
    range: Range,
    context: ActionContext,
}

#[derive(Deserialize)]
struct ActionContext {
    only: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct WatchedParams {
    changes: Vec<DocumentId>,
}

/// Serve framed JSON-RPC until `exit` or EOF. No source files are written.
/// The input/output abstraction also permits deterministic protocol replay tests.
pub fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    cwd: &Path,
    options: LoadOptions,
) -> Result<u8, String> {
    options
        .overrides
        .validate()
        .map_err(|error| error.to_string())?;
    let mut server = Server {
        root: cwd.to_owned(),
        options,
        lifecycle: Lifecycle::New,
        buffers: BTreeMap::new(),
        published: BTreeSet::new(),
        related_information: false,
        version_support: false,
        code_description: false,
        quick_fixes: false,
        watch_registration: false,
    };
    while let Some(body) = transport::read(&mut input).map_err(|error| error.to_string())? {
        let message: Value = match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(error) => {
                send(
                    &mut output,
                    error_response(Value::Null, -32700, &error.to_string()),
                )?;
                continue;
            }
        };
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || id
                .as_ref()
                .is_some_and(|id| !id.is_string() && !id.is_i64())
            || method.is_none()
        {
            // Responses to optional client registration are not requests.
            if method.is_none()
                && (message.get("result").is_some() || message.get("error").is_some())
            {
                continue;
            }
            send(
                &mut output,
                error_response(
                    id.unwrap_or(Value::Null),
                    -32600,
                    "Invalid JSON-RPC request.",
                ),
            )?;
            continue;
        }
        let method = method.unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method == "exit" && id.is_none() {
            return Ok(if server.lifecycle == Lifecycle::Shutdown {
                0
            } else {
                1
            });
        }
        if let Some(id) = id {
            let response = match server.request(method, params) {
                Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                Err((code, message)) => error_response(id, code, &message),
            };
            send(&mut output, response)?;
        } else if let Err(error) = server.notification(method, params, &mut output) {
            send(
                &mut output,
                notification("window/logMessage", json!({"type":1,"message":error})),
            )?;
        }
    }
    Ok(if server.lifecycle == Lifecycle::Shutdown {
        0
    } else {
        1
    })
}

type RpcResult = Result<Value, (i32, String)>;

impl Server {
    fn request(&mut self, method: &str, params: Value) -> RpcResult {
        if method == "initialize" {
            if self.lifecycle != Lifecycle::New {
                return Err((-32600, "The server has already been initialized.".into()));
            }
            return self.initialize(params).map_err(|error| (-32602, error));
        }
        if matches!(self.lifecycle, Lifecycle::New | Lifecycle::Initializing) {
            return Err((-32002, "Server not initialized.".into()));
        }
        if self.lifecycle == Lifecycle::Shutdown {
            return Err((-32600, "The server has shut down; send exit.".into()));
        }
        match method {
            "shutdown" => {
                self.lifecycle = Lifecycle::Shutdown;
                Ok(Value::Null)
            }
            "textDocument/codeAction" => self.code_actions(params).map_err(|error| (-32602, error)),
            _ => Err((-32601, format!("Unsupported method: {method}"))),
        }
    }

    fn initialize(&mut self, params: Value) -> Result<Value, String> {
        if !params.is_object() {
            return Err("initialize requires an object of parameters.".into());
        }
        let folders = params.get("workspaceFolders").and_then(Value::as_array);
        if folders.is_some_and(|folders| folders.len() > 1) {
            return Err(
                "seiso serves one workspace per process; start a server for each folder.".into(),
            );
        }
        let root = if let Some(folder) = folders.and_then(|folders| folders.first()) {
            path_from_uri(
                folder
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or("Workspace folder needs a file URI.")?,
            )?
        } else if let Some(uri) = params.get("rootUri").and_then(Value::as_str) {
            path_from_uri(uri)?
        } else if let Some(path) = params.get("rootPath").and_then(Value::as_str) {
            PathBuf::from(path)
        } else {
            self.root.clone()
        };
        if !root.is_absolute() || !root.is_dir() {
            return Err("The workspace root must be an existing absolute directory.".into());
        }
        let workspace = Workspace::discover(&root, self.options.config.as_deref())
            .map_err(|error| error.to_string())?;
        self.root = workspace.root;
        let supports =
            |pointer: &str| params.pointer(pointer).and_then(Value::as_bool) == Some(true);
        self.related_information =
            supports("/capabilities/textDocument/publishDiagnostics/relatedInformation");
        self.version_support =
            supports("/capabilities/textDocument/publishDiagnostics/versionSupport");
        self.code_description =
            supports("/capabilities/textDocument/publishDiagnostics/codeDescriptionSupport");
        self.watch_registration =
            supports("/capabilities/workspace/didChangeWatchedFiles/dynamicRegistration");
        self.quick_fixes = params
            .pointer("/capabilities/textDocument/codeAction/codeActionLiteralSupport")
            .is_some()
            && supports("/capabilities/workspace/workspaceEdit/documentChanges");
        self.lifecycle = Lifecycle::Initializing;
        Ok(json!({
            "capabilities": {
                "positionEncoding":"utf-16",
                "textDocumentSync":{"openClose":true,"change":2,"save":{"includeText":false}},
                "codeActionProvider": if self.quick_fixes { json!({"codeActionKinds":["quickfix"]}) } else { json!(false) },
                "workspace":{"workspaceFolders":{"supported":false,"changeNotifications":false}}
            },
            "serverInfo":{"name":"seiso","version":env!("CARGO_PKG_VERSION")}
        }))
    }

    fn notification(
        &mut self,
        method: &str,
        params: Value,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if method == "initialized" && self.lifecycle == Lifecycle::Initializing {
            self.lifecycle = Lifecycle::Running;
            if self.watch_registration {
                send(
                    output,
                    json!({
                        "jsonrpc":"2.0","id":"seiso/watch-files","method":"client/registerCapability",
                        "params":{"registrations":[{
                            "id":"seiso/workspace-files","method":"workspace/didChangeWatchedFiles",
                            "registerOptions":{"watchers":[{"globPattern":"**/*","kind":7}]}
                        }]}
                    }),
                )?;
            }
            return Ok(());
        }
        if self.lifecycle != Lifecycle::Running {
            return Ok(());
        }
        match method {
            "textDocument/didOpen" => {
                let params: OpenParams = decode(params)?;
                let doc = params.text_document;
                let path = self.document_path(&doc.uri)?;
                if !workspace::is_markdown(&path) {
                    return Ok(());
                }
                if self.buffers.contains_key(&path) {
                    return Err("Document is already open; close it before reopening.".into());
                }
                self.buffers.insert(
                    path,
                    Buffer {
                        uri: doc.uri,
                        version: doc.version,
                        text: doc.text,
                        synchronized: true,
                    },
                );
                self.refresh(output)
            }
            "textDocument/didChange" => {
                let params: ChangeParams = decode(params)?;
                let path = self.document_path(&params.text_document.uri)?;
                let buffer = self
                    .buffers
                    .get_mut(&path)
                    .ok_or("Change received for a document that is not open.")?;
                if params.text_document.version <= buffer.version {
                    return Err("Ignored an out-of-order document version.".into());
                }
                // Track receipt even if this edit fails, so delayed full text cannot
                // recover the buffer to a version older than an already seen event.
                buffer.version = params.text_document.version;
                let changes = &params.content_changes;
                // A full replacement is the only way to recover from a rejected range.
                let first = if buffer.synchronized {
                    0
                } else {
                    changes
                        .iter()
                        .position(|change| change.range.is_none())
                        .ok_or("Document is out of sync; send full text or close and reopen it.")?
                };
                match apply_changes(&buffer.text, &changes[first..]) {
                    Ok(text) => {
                        buffer.text = text;
                        buffer.synchronized = true;
                    }
                    Err(error) => {
                        buffer.synchronized = false;
                        self.clear(output)?;
                        return Err(format!(
                            "{error} Send full text or close and reopen the document."
                        ));
                    }
                }
                self.refresh(output)
            }
            "textDocument/didClose" => {
                let params: DocumentParams = decode(params)?;
                let path = self.document_path(&params.text_document.uri)?;
                self.buffers.remove(&path);
                self.refresh(output)
            }
            "textDocument/didSave" => {
                let params: DocumentParams = decode(params)?;
                self.document_path(&params.text_document.uri)?;
                self.refresh(output)
            }
            "workspace/didChangeWatchedFiles" => {
                let params: WatchedParams = decode(params)?;
                let relevant = params.changes.iter().any(|change| {
                    path_from_uri(&change.uri).is_ok_and(|path| {
                        path.strip_prefix(&self.root).is_ok_and(|relative| {
                            !relative.components().any(|part| {
                                matches!(part.as_os_str().to_str(), Some(".git" | ".seiso_cache"))
                            })
                        })
                    })
                });
                if relevant {
                    self.refresh(output)
                } else {
                    Ok(())
                }
            }
            "workspace/didChangeConfiguration" => self.refresh(output),
            _ => Ok(()),
        }
    }

    fn document_path(&self, uri: &str) -> Result<PathBuf, String> {
        workspace::workspace_path(&self.root, &path_from_uri(uri)?)
    }

    fn analyze(&self) -> Result<Analysis, String> {
        if self.buffers.values().any(|buffer| !buffer.synchronized) {
            return Err(
                "An open document is out of sync; send full text or close and reopen it.".into(),
            );
        }
        let overlays = self
            .buffers
            .iter()
            .map(|(path, buffer)| (path.clone(), buffer.text.clone()))
            .collect();
        let snapshot =
            workspace::load_with_overlays(&self.root, &self.options, LoadScope::Check, &overlays)?;
        analysis::check(snapshot, &self.options.overrides)
    }

    fn refresh(&mut self, output: &mut impl Write) -> Result<(), String> {
        let analysis = match self.analyze() {
            Ok(analysis) => analysis,
            Err(error) => {
                self.clear(output)?;
                return Err(error);
            }
        };
        let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        for buffer in self.buffers.values() {
            grouped.insert(buffer.uri.clone(), Vec::new());
        }
        for diagnostic in &analysis.diagnostics {
            let uri = self.uri(&analysis.snapshot.index.root.join(&diagnostic.filename))?;
            grouped
                .entry(uri)
                .or_default()
                .push(self.diagnostic(&analysis, diagnostic)?);
        }
        for uri in &self.published {
            grouped.entry(uri.clone()).or_default();
        }
        let mut published = BTreeSet::new();
        for (uri, diagnostics) in grouped {
            if !diagnostics.is_empty() {
                published.insert(uri.clone());
            }
            let mut params = json!({"uri":uri,"diagnostics":diagnostics});
            if self.version_support
                && let Some(buffer) = self.buffers.values().find(|buffer| buffer.uri == uri)
            {
                params["version"] = json!(buffer.version);
            }
            send(
                output,
                notification("textDocument/publishDiagnostics", params),
            )?;
        }
        self.published = published;
        if !analysis.snapshot.errors.is_empty() {
            let errors = analysis
                .snapshot
                .errors
                .iter()
                .map(|error| format!("{}: {}", error.filename, error.message))
                .collect::<Vec<_>>()
                .join("\n");
            send(
                output,
                notification(
                    "window/logMessage",
                    json!({"type":1,"message":format!("Incomplete seiso check; quick fixes are unavailable.\n{errors}")}),
                ),
            )?;
        }
        Ok(())
    }

    fn clear(&mut self, output: &mut impl Write) -> Result<(), String> {
        for uri in std::mem::take(&mut self.published) {
            send(
                output,
                notification(
                    "textDocument/publishDiagnostics",
                    json!({"uri":uri,"diagnostics":[]}),
                ),
            )?;
        }
        Ok(())
    }

    fn uri(&self, path: &Path) -> Result<String, String> {
        self.buffers
            .get(path)
            .map(|buffer| Ok(buffer.uri.clone()))
            .unwrap_or_else(|| uri_from_path(path))
    }

    fn diagnostic(&self, analysis: &Analysis, diagnostic: &Diagnostic) -> Result<Value, String> {
        let index = &analysis.snapshot.index;
        let file = index
            .file(&diagnostic.filename)
            .ok_or("Diagnostic source is missing from the snapshot.")?;
        let mut value = json!({
            "range":TextMap::new(&file.document.source).range(diagnostic.byte_range),
            "severity":2,
            "code":diagnostic.code,
            "source":"seiso",
            "message":format!("{}\n{}", diagnostic.message, diagnostic.suggestion)
        });
        if self.code_description
            && let Some(url) = &diagnostic.url
        {
            value["codeDescription"] = json!({"href":url});
        }
        if self.related_information {
            let mut related = Vec::new();
            for location in &diagnostic.related {
                // Without its source, a scalar column cannot safely become UTF-16.
                if let Some(file) = index.file(&location.filename) {
                    related.push(json!({
                        "location":{"uri":self.uri(&file.path)?,"range":TextMap::new(&file.document.source).range(location.byte_range)},
                        "message":location.message
                    }));
                }
            }
            if !related.is_empty() {
                value["relatedInformation"] = json!(related);
            }
        }
        Ok(value)
    }

    fn code_actions(&self, params: Value) -> Result<Value, String> {
        let params: ActionParams = decode(params)?;
        if !self.quick_fixes
            || params.context.only.as_ref().is_some_and(|kinds| {
                !kinds
                    .iter()
                    .any(|kind| kind.is_empty() || kind == "quickfix")
            })
        {
            return Ok(json!([]));
        }
        let path = self.document_path(&params.text_document.uri)?;
        let Some(buffer) = self.buffers.get(&path).filter(|buffer| buffer.synchronized) else {
            return Ok(json!([]));
        };
        // Recheck all inputs: even suppression fixes can depend on another file or policy.
        let analysis = self.analyze()?;
        if !analysis.snapshot.errors.is_empty() {
            return Ok(json!([]));
        }
        let map = TextMap::new(&buffer.text);
        let requested = map.span(params.range)?;
        let filename = workspace::relative(&analysis.snapshot.index.root, &path);
        let mut actions = Vec::new();
        let mut seen = BTreeSet::new();
        for diagnostic in analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.filename == filename)
        {
            if diagnostic.byte_range.end < requested.start
                || diagnostic.byte_range.start > requested.end
            {
                continue;
            }
            let Some(fix) = diagnostic
                .fix
                .as_ref()
                .filter(|fix| fix.applicability == Applicability::Safe && !fix.edits.is_empty())
            else {
                continue;
            };
            if !seen.insert(fix.clone()) {
                continue;
            }
            crate::rules::fixes::apply_fixes(&buffer.text, std::slice::from_ref(diagnostic))?;
            let edits: Vec<_> = fix
                .edits
                .iter()
                .map(|edit| json!({"range":map.range(edit.byte_range),"newText":edit.content}))
                .collect();
            actions.push(json!({
                "title":fix.message,
                "kind":"quickfix",
                "diagnostics":[self.diagnostic(&analysis, diagnostic)?],
                "edit":{"documentChanges":[{"textDocument":{"uri":buffer.uri,"version":buffer.version},"edits":edits}]}
            }));
        }
        Ok(json!(actions))
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("Invalid LSP parameters: {error}"))
}

fn path_from_uri(uri: &str) -> Result<PathBuf, String> {
    let url = Url::parse(uri).map_err(|error| format!("Invalid document URI: {error}"))?;
    if url.scheme() != "file" || url.query().is_some() || url.fragment().is_some() {
        return Err("Only file URIs without a query or fragment are supported.".into());
    }
    url.to_file_path()
        .map_err(|_| "Cannot convert the file URI to a local path.".into())
}

fn uri_from_path(path: &Path) -> Result<String, String> {
    Url::from_file_path(path)
        .map(|url| url.to_string())
        .map_err(|_| format!("Cannot encode file URI: {}", path.display()))
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","method":method,"params":params})
}

fn error_response(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn send(output: &mut impl Write, value: Value) -> Result<(), String> {
    transport::write(output, &value).map_err(|error| error.to_string())
}
