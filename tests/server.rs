use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use seiso::workspace::LoadOptions;
use serde_json::{Value, json};
use tempfile::TempDir;
use url::Url;

fn workspace(config: &str) -> TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("seiso.toml"), config).unwrap();
    root
}

fn uri(root: &Path, name: &str) -> String {
    Url::from_file_path(root.join(name)).unwrap().to_string()
}

fn notify(method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","method":method,"params":params})
}

fn request(id: i32, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

fn initialize(root: &Path) -> Value {
    request(
        1,
        "initialize",
        json!({
            "rootUri": uri(root, ""),
            "capabilities": {
                "workspace": {"workspaceEdit":{"documentChanges":true},"didChangeWatchedFiles":{"dynamicRegistration":true}},
                "textDocument": {
                    "codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix"]}}},
                    "publishDiagnostics":{"versionSupport":true,"relatedInformation":true,"codeDescriptionSupport":true}
                }
            }
        }),
    )
}

fn open(root: &Path, name: &str, text: &str) -> Value {
    notify(
        "textDocument/didOpen",
        json!({"textDocument":{"uri":uri(root, name),"languageId":"markdown","version":1,"text":text}}),
    )
}

fn change(root: &Path, name: &str, version: i32, changes: Value) -> Value {
    notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":uri(root, name),"version":version},"contentChanges":changes}),
    )
}

fn close(root: &Path, name: &str) -> Value {
    notify(
        "textDocument/didClose",
        json!({"textDocument":{"uri":uri(root, name)}}),
    )
}

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start":{"line":line,"character":start},"end":{"line":line,"character":end}})
}

fn action(root: &Path, name: &str, id: i32) -> Value {
    request(
        id,
        "textDocument/codeAction",
        json!({"textDocument":{"uri":uri(root, name)},"range":range(3,0,1000),"context":{"diagnostics":[]}}),
    )
}

fn write_message(output: &mut impl Write, message: &Value) {
    let bytes = serde_json::to_vec(message).unwrap();
    write!(output, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
    output.write_all(&bytes).unwrap();
    output.flush().unwrap();
}

fn read_message(input: &mut impl BufRead) -> Option<Value> {
    let mut header = String::new();
    if input.read_line(&mut header).unwrap() == 0 {
        return None;
    }
    let length: usize = header
        .trim()
        .strip_prefix("Content-Length: ")
        .unwrap()
        .parse()
        .unwrap();
    let mut blank = String::new();
    input.read_line(&mut blank).unwrap();
    assert_eq!(blank, "\r\n");
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).unwrap();
    Some(serde_json::from_slice(&bytes).unwrap())
}

fn replay_raw(root: &Path, messages: &[Value]) -> (u8, Vec<Value>) {
    let mut input = Vec::new();
    for message in messages {
        write_message(&mut input, message);
    }
    let mut output = Vec::new();
    let code = seiso::server::serve(
        input.as_slice(),
        &mut output,
        root,
        LoadOptions {
            no_cache: true,
            ..LoadOptions::default()
        },
    )
    .unwrap();
    let mut input = output.as_slice();
    let mut messages = Vec::new();
    while let Some(value) = read_message(&mut input) {
        messages.push(value);
    }
    (code, messages)
}

fn replay(root: &Path, events: Vec<Value>) -> Vec<Value> {
    let mut messages = vec![initialize(root), notify("initialized", json!({}))];
    messages.extend(events);
    messages.extend([
        request(999, "shutdown", Value::Null),
        notify("exit", Value::Null),
    ]);
    let (code, messages) = replay_raw(root, &messages);
    assert_eq!(code, 0);
    messages
}

fn response(messages: &[Value], id: i32) -> &Value {
    messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(id)))
        .unwrap()
}

fn publications<'a>(messages: &'a [Value], uri: &str) -> Vec<&'a Value> {
    messages
        .iter()
        .filter(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["uri"] == uri
        })
        .map(|message| &message["params"])
        .collect()
}

fn codes(publication: &Value) -> Vec<&str> {
    publication["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["code"].as_str().unwrap())
        .collect()
}

#[test]
fn lifecycle_negotiates_capabilities_and_rejects_invalid_requests() {
    let root = workspace("");
    let messages = vec![
        request(0, "shutdown", Value::Null),
        initialize(root.path()),
        request(2, "textDocument/codeAction", json!({})),
        notify("initialized", json!({})),
        request(3, "initialize", json!({})),
        request(4, "unknown/method", json!({})),
        request(5, "textDocument/codeAction", json!({})),
        request(6, "shutdown", Value::Null),
        request(7, "shutdown", Value::Null),
        notify("exit", Value::Null),
    ];
    let (code, messages) = replay_raw(root.path(), &messages);
    assert_eq!(code, 0);
    for (id, error) in [
        (0, -32002),
        (2, -32002),
        (3, -32600),
        (4, -32601),
        (5, -32602),
        (7, -32600),
    ] {
        assert_eq!(response(&messages, id)["error"]["code"], error);
    }
    let capabilities = &response(&messages, 1)["result"]["capabilities"];
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert_eq!(capabilities["textDocumentSync"]["change"], 2);
    assert_eq!(
        capabilities["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix"])
    );
    assert!(
        messages
            .iter()
            .any(|message| message["method"] == "client/registerCapability")
    );
    assert_eq!(replay_raw(root.path(), &[notify("exit", Value::Null)]).0, 1);
    assert_eq!(replay_raw(root.path(), &[]).0, 1);
}

#[test]
fn unsupported_multi_root_and_remote_uris_are_explicit_errors() {
    let root = workspace("");
    for params in [
        json!({"rootUri":"https://example.com/docs"}),
        json!({"workspaceFolders":[{"uri":uri(root.path(), "")},{"uri":uri(root.path(), "")}]}),
    ] {
        let (_, messages) = replay_raw(root.path(), &[request(1, "initialize", params)]);
        assert_eq!(response(&messages, 1)["error"]["code"], -32602);
    }
}

#[test]
fn minimal_clients_get_only_supported_diagnostic_fields() {
    let root = workspace("[lint]\nselect = ['KND001']\n");
    let (_, messages) = replay_raw(
        root.path(),
        &[
            request(
                1,
                "initialize",
                json!({"rootUri":uri(root.path(),""),"capabilities":{}}),
            ),
            notify("initialized", json!({})),
            open(root.path(), "new.md", "# New\n"),
            action(root.path(), "new.md", 2),
        ],
    );
    assert_eq!(
        response(&messages, 1)["result"]["capabilities"]["codeActionProvider"],
        false
    );
    assert_eq!(response(&messages, 2)["result"], json!([]));
    let publication = publications(&messages, &uri(root.path(), "new.md"))[0];
    assert!(publication.get("version").is_none());
    assert!(
        publication["diagnostics"][0]
            .get("codeDescription")
            .is_none()
    );
}

#[test]
fn open_change_and_close_use_memory_then_restore_disk_without_writing_it() {
    let root = workspace("[lint]\nselect = ['KND001']\n");
    std::fs::write(root.path().join("doc.md"), "# Disk\n").unwrap();
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "doc.md", "---\nkind: howto\n---\n# Memory\n"),
            change(
                root.path(),
                "doc.md",
                2,
                json!([{"text":"# Missing kind\n"}]),
            ),
            close(root.path(), "doc.md"),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "doc.md"));
    assert!(codes(reports[0]).is_empty());
    assert_eq!(codes(reports[1]), ["KND001"]);
    assert_eq!(reports[1]["version"], 2);
    assert_eq!(codes(reports[2]), ["KND001"]);
    assert!(reports[2].get("version").is_none());
    assert_eq!(
        std::fs::read_to_string(root.path().join("doc.md")).unwrap(),
        "# Disk\n"
    );
}

#[test]
fn unsaved_documents_participate_in_links_and_cross_file_refresh() {
    let root = workspace("preview = true\n[lint]\nselect = ['LNK001', 'LNK002']\n");
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "from.md", "[Target](target.md#fresh)\n"),
            open(root.path(), "target.md", "# Fresh\n"),
            change(root.path(), "target.md", 2, json!([{"text":"# Renamed\n"}])),
            close(root.path(), "target.md"),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "from.md"));
    assert_eq!(codes(reports[0]), ["LNK001"]);
    assert!(codes(reports[1]).is_empty());
    assert_eq!(codes(reports[2]), ["LNK002"]);
    assert_eq!(
        reports[2]["diagnostics"][0]["relatedInformation"][0]["location"]["uri"],
        uri(root.path(), "target.md")
    );
    assert!(
        reports[2]["diagnostics"][0]["codeDescription"]["href"]
            .as_str()
            .unwrap()
            .ends_with("/docs/rules/LNK002.md")
    );
    assert_eq!(codes(reports[3]), ["LNK001"]);
    assert!(!root.path().join("target.md").exists());
}

#[test]
fn removing_an_unsaved_document_clears_its_diagnostics() {
    let root = workspace("[lint]\nselect = ['KND001']\n");
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "new.md", "# New"),
            close(root.path(), "new.md"),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "new.md"));
    assert_eq!(codes(reports[0]), ["KND001"]);
    assert!(codes(reports.last().unwrap()).is_empty());
}

#[test]
fn incremental_unicode_and_crlf_edits_refresh_the_right_ranges() {
    let root = workspace("[lint]\nselect = ['LNK001']\n");
    let source = "中文🦀 [坏](missing.md)\r\n";
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "日本 空格#.md", source),
            change(
                root.path(),
                "日本 空格#.md",
                2,
                json!([{"range":range(0,5,20),"rangeLength":15,"text":"fixed"}]),
            ),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "日本 空格#.md"));
    assert_eq!(codes(reports[0]), ["LNK001"]);
    assert_eq!(
        reports[0]["diagnostics"][0]["range"]["start"]["character"],
        5
    );
    assert!(codes(reports[1]).is_empty());
    assert_eq!(reports[1]["version"], 2);
}

#[test]
fn stale_changes_are_ignored_and_invalid_edits_require_full_text_recovery() {
    let root = workspace("[lint]\nselect = ['LNK001']\n");
    let source = "🦀 [bad](missing.md)";
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "doc.md", source),
            change(root.path(), "doc.md", 1, json!([{"text":"clean"}])),
            change(
                root.path(),
                "doc.md",
                2,
                json!([{"range":range(0,1,2),"text":"a"}]),
            ),
            change(
                root.path(),
                "doc.md",
                3,
                json!([{"range":range(0,0,2),"text":"a"}]),
            ),
            change(root.path(), "doc.md", 4, json!([{"text":source}])),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "doc.md"));
    assert_eq!(reports.len(), 3);
    assert_eq!(codes(reports[0]), ["LNK001"]);
    assert!(codes(reports[1]).is_empty());
    assert_eq!(codes(reports[2]), ["LNK001"]);
    assert_eq!(reports[2]["version"], 4);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["method"] == "window/logMessage")
            .count(),
        3
    );
}

#[test]
fn ignored_and_excluded_buffers_do_not_pollute_other_documents() {
    let root = workspace("exclude = ['excluded/**']\n[lint]\nselect = ['KND001']\n");
    std::fs::write(root.path().join(".gitignore"), "ignored/\n").unwrap();
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "ignored/new.md", "# Ignored"),
            open(root.path(), "excluded/new.md", "# Excluded"),
            open(root.path(), "included.md", "# Included"),
        ],
    );
    for name in ["ignored/new.md", "excluded/new.md"] {
        assert!(
            publications(&messages, &uri(root.path(), name))
                .iter()
                .all(|report| codes(report).is_empty())
        );
    }
    assert_eq!(
        codes(publications(&messages, &uri(root.path(), "included.md"))[0]),
        ["KND001"]
    );
    assert!(
        !messages
            .iter()
            .any(|message| message["method"] == "window/logMessage")
    );
}

#[test]
fn safe_quick_fixes_are_versioned_deduplicated_and_never_written() {
    let root = workspace("[lint]\nselect = ['LNK001','KND001','SUP002']\n");
    let source = "---\nkind: howto\n---\n<!-- seiso: allow-file LNK001, KND001 -- Historical. -->\n\n# Setup\n";
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "doc.md", source),
            action(root.path(), "doc.md", 2),
        ],
    );
    let actions = response(&messages, 2)["result"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    let edit = &actions[0]["edit"]["documentChanges"][0];
    assert_eq!(
        edit["textDocument"],
        json!({"uri":uri(root.path(),"doc.md"),"version":1})
    );
    assert_eq!(edit["edits"][0]["newText"], "");
    assert_eq!(edit["edits"][0]["range"]["start"]["line"], 3);
    assert_eq!(edit["edits"][0]["range"]["end"]["line"], 4);
    assert!(!root.path().join("doc.md").exists());
}

#[test]
fn quick_fixes_respect_requested_kind_and_range() {
    let root = workspace("[lint]\nselect = ['LNK001','SUP002']\n");
    let source =
        "---\nkind: howto\n---\n<!-- seiso: allow-file LNK001 -- Historical. -->\n\n# Setup\n";
    let mut refactor = action(root.path(), "doc.md", 2);
    refactor["params"]["context"]["only"] = json!(["refactor"]);
    let mut elsewhere = action(root.path(), "doc.md", 3);
    elsewhere["params"]["range"] = range(0, 0, 1);
    let messages = replay(
        root.path(),
        vec![open(root.path(), "doc.md", source), refactor, elsewhere],
    );
    assert_eq!(response(&messages, 2)["result"], json!([]));
    assert_eq!(response(&messages, 3)["result"], json!([]));
}

#[test]
fn incomplete_workspace_checks_never_offer_fixes() {
    let root = workspace("[lint]\nselect = ['LNK001','SUP002']\n");
    std::fs::write(root.path().join("invalid.md"), [0xff]).unwrap();
    let source =
        "---\nkind: howto\n---\n<!-- seiso: allow-file LNK001 -- Historical. -->\n\n# Setup\n";
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "doc.md", source),
            action(root.path(), "doc.md", 2),
        ],
    );
    assert_eq!(response(&messages, 2)["result"], json!([]));
    assert!(messages.iter().any(|message| {
        message["params"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("Incomplete seiso check"))
    }));
}

#[test]
fn malformed_json_can_be_followed_by_a_valid_message() {
    let root = workspace("");
    let mut input = b"Content-Length: 1\r\n\r\n{".to_vec();
    write_message(&mut input, &initialize(root.path()));
    let mut output = Vec::new();
    seiso::server::serve(
        input.as_slice(),
        &mut output,
        root.path(),
        LoadOptions::default(),
    )
    .unwrap();
    let mut reader = output.as_slice();
    assert_eq!(read_message(&mut reader).unwrap()["error"]["code"], -32700);
    assert!(read_message(&mut reader).unwrap().get("result").is_some());
}

#[test]
fn desynchronization_never_accepts_older_full_text() {
    let root = workspace("[lint]\nselect = ['LNK001']\n");
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "doc.md", "🦀 [bad](missing.md)"),
            change(
                root.path(),
                "doc.md",
                5,
                json!([{"range":range(0,1,2),"text":"a"}]),
            ),
            change(root.path(), "doc.md", 4, json!([{"text":"clean"}])),
            change(
                root.path(),
                "doc.md",
                6,
                json!([{"text":"[bad](missing.md)"}]),
            ),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "doc.md"));
    assert_eq!(reports.len(), 3);
    assert!(codes(reports[1]).is_empty());
    assert_eq!(reports[2]["version"], 6);
    assert_eq!(codes(reports[2]), ["LNK001"]);
}

#[test]
fn nested_policy_and_preview_opt_in_apply_to_unsaved_buffers() {
    let root = workspace("[lint]\nselect = ['KND001']\n");
    std::fs::create_dir(root.path().join("nested")).unwrap();
    std::fs::write(
        root.path().join("nested/seiso.toml"),
        "preview = true\n[lint]\nselect = ['LNK002']\n",
    )
    .unwrap();
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "root.md", "[Missing](target.md#missing)"),
            open(
                root.path(),
                "nested/doc.md",
                "[Missing](../target.md#missing)",
            ),
            open(root.path(), "target.md", "# Heading"),
        ],
    );
    assert_eq!(
        codes(
            publications(&messages, &uri(root.path(), "root.md"))
                .last()
                .unwrap()
        ),
        ["KND001"]
    );
    assert_eq!(
        codes(
            publications(&messages, &uri(root.path(), "nested/doc.md"))
                .last()
                .unwrap()
        ),
        ["LNK002"]
    );
}

#[test]
fn closing_an_existing_buffer_restores_disk_anchors_for_incoming_links() {
    let root = workspace("preview = true\n[lint]\nselect = ['LNK002']\n");
    std::fs::write(root.path().join("target.md"), "# Saved").unwrap();
    std::fs::write(root.path().join("incoming.md"), "[Target](target.md#saved)").unwrap();
    let messages = replay(
        root.path(),
        vec![
            open(root.path(), "target.md", "# Unsaved"),
            close(root.path(), "target.md"),
        ],
    );
    let reports = publications(&messages, &uri(root.path(), "incoming.md"));
    assert_eq!(codes(reports[0]), ["LNK002"]);
    assert!(codes(reports[1]).is_empty());
}

/// Exercise a real long-lived process while changing files between messages.
struct Client {
    child: Child,
    input: ChildStdin,
    output: Receiver<Value>,
    sequence: i32,
    publications: BTreeMap<String, Value>,
    logs: Vec<String>,
}

impl Client {
    fn new(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_seiso"))
            .args(["server", "--no-cache"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = read_message(&mut reader) {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output,
            sequence: 10,
            publications: BTreeMap::new(),
            logs: Vec::new(),
        };
        write_message(&mut client.input, &initialize(root));
        client.receive(1);
        client.notify(notify("initialized", json!({})));
        client
    }

    fn receive(&mut self, id: i32) -> Value {
        loop {
            let message = self
                .output
                .recv_timeout(Duration::from_secs(20))
                .expect("LSP server must respond without closing stdin");
            if message["id"] == id {
                return message;
            }
            if message["method"] == "textDocument/publishDiagnostics" {
                self.publications.insert(
                    message["params"]["uri"].as_str().unwrap().into(),
                    message["params"].clone(),
                );
            } else if message["method"] == "window/logMessage" {
                self.logs
                    .push(message["params"]["message"].as_str().unwrap().into());
            } else if message["method"] == "client/registerCapability" {
                write_message(
                    &mut self.input,
                    &json!({"jsonrpc":"2.0","id":message["id"],"result":null}),
                );
            }
        }
    }

    fn notify(&mut self, message: Value) {
        write_message(&mut self.input, &message);
        self.sequence += 1;
        write_message(
            &mut self.input,
            &request(self.sequence, "test/barrier", json!({})),
        );
        assert_eq!(self.receive(self.sequence)["error"]["code"], -32601);
    }

    fn watch(&mut self, root: &Path, name: &str) {
        self.notify(notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes":[{"uri":uri(root,name),"type":2}]}),
        ));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn live_watched_file_changes_refresh_links_config_errors_and_recovery() {
    let root = workspace("[lint]\nselect = ['LNK001']\n");
    let mut client = Client::new(root.path());
    client.notify(open(root.path(), "doc.md", "[file](asset.txt)\n"));
    let doc_uri = uri(root.path(), "doc.md");
    assert_eq!(codes(&client.publications[&doc_uri]), ["LNK001"]);
    std::fs::write(root.path().join("asset.txt"), "exists").unwrap();
    client.watch(root.path(), "asset.txt");
    assert!(codes(&client.publications[&doc_uri]).is_empty());
    std::fs::remove_file(root.path().join("asset.txt")).unwrap();
    client.watch(root.path(), "asset.txt");
    assert_eq!(codes(&client.publications[&doc_uri]), ["LNK001"]);
    std::fs::write(root.path().join("seiso.toml"), "[invalid").unwrap();
    client.watch(root.path(), "seiso.toml");
    assert!(codes(&client.publications[&doc_uri]).is_empty());
    assert!(!client.logs.is_empty());
    std::fs::write(
        root.path().join("seiso.toml"),
        "[lint]\nselect = ['KND001']\n",
    )
    .unwrap();
    client.watch(root.path(), "seiso.toml");
    assert_eq!(codes(&client.publications[&doc_uri]), ["KND001"]);
    // Server-generated cache events must not trigger a feedback loop.
    client.publications.clear();
    client.watch(root.path(), ".seiso_cache/example.cache");
    assert!(client.publications.is_empty());
    write_message(&mut client.input, &request(100, "shutdown", Value::Null));
    assert_eq!(client.receive(100)["result"], Value::Null);
    write_message(&mut client.input, &notify("exit", Value::Null));
    assert!(client.child.wait().unwrap().success());
}

#[test]
fn quick_fixes_recheck_files_changed_since_the_last_diagnostics() {
    let root = workspace("[lint]\nselect = ['LNK001','SUP002']\n");
    let mut client = Client::new(root.path());
    let source =
        "---\nkind: howto\n---\n<!-- seiso: allow-file LNK001 -- Historical. -->\n\n# Setup\n";
    client.notify(open(root.path(), "doc.md", source));
    assert_eq!(
        codes(&client.publications[&uri(root.path(), "doc.md")]),
        ["SUP002"]
    );
    std::fs::write(
        root.path().join("seiso.toml"),
        "[lint]\nselect = ['LNK001']\n",
    )
    .unwrap();
    write_message(&mut client.input, &action(root.path(), "doc.md", 50));
    assert_eq!(client.receive(50)["result"], json!([]));
}
