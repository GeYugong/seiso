---
kind: reference
---

# Language server

`seiso server` serves the [Language Server Protocol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
over stdin and stdout. The [editor setup guide](../guides/integrations.md#editors)
describes client configuration. The implementation is in
[`src/server/`](../../src/server/).

## Launch and workspace

The command accepts `--config PATH`, `--preview`, `--select`, `--extend-select`,
and `--no-cache`. Selection has the same meaning as in
[`seiso check`](../guides/checking.md). Relative explicit configuration paths
resolve against the process's starting directory.

Each process serves one workspace. Initialization chooses its directory from
the sole `workspaceFolders` entry, then `rootUri`, then the legacy `rootPath`,
then the process's starting directory. Normal configuration discovery establishes
the repository root. Multiple workspace folders are rejected with an explanation
to start one process per folder. Dynamic workspace-folder changes are unsupported.

Documents use absolute `file:` URIs. Spaces, percent signs, Unicode names, and
Windows drive letters are handled through URI conversion. Queries, fragments,
non-file schemes, and paths resolving outside the workspace are rejected.
Existing filesystem aliases resolve through the same containment check as CLI
stdin documents. Markdown buffers use `.md` or `.markdown` paths.

## Protocol surface

| Message | Behavior |
| --- | --- |
| `initialize`, `initialized` | Establish the workspace and negotiate capabilities |
| `textDocument/didOpen` | Add a versioned in-memory source and refresh diagnostics |
| `textDocument/didChange` | Apply full or incremental changes and refresh diagnostics |
| `textDocument/didSave` | Reload disk dependencies and policy; keep open text authoritative |
| `textDocument/didClose` | Remove the overlay, restore disk content, and refresh diagnostics |
| `workspace/didChangeWatchedFiles` | Refresh relevant workspace files and configuration |
| `workspace/didChangeConfiguration` | Reload configuration files from disk |
| `textDocument/codeAction` | Reanalyze inputs and return safe, versioned quick fixes |
| `shutdown`, `exit` | Acknowledge shutdown, then terminate the process |

The server advertises incremental text synchronization and UTF-16 positions.
Changes within one notification apply sequentially against the preceding result,
and the entire notification commits atomically. Chinese text, surrogate pairs,
combining characters, LF, CRLF, and bare CR retain their source positions.
Out-of-order versions are ignored. Invalid edit ranges invalidate the buffer,
clear published findings, and suspend analysis until full text or a close/open
cycle restores synchronization. A save notification alone cannot restore it.

Clients supporting dynamic file watchers receive a registration covering
workspace file changes, including non-Markdown link targets. Git metadata and
seiso cache events do not trigger refreshes. Clients without this capability
refresh on document events or may send watched-file notifications themselves.
Configuration supplied in notification payloads is not applied: policy remains
in the repository's configuration files.

## Analysis and diagnostics

Every refresh uses the same rules, selection, nested configurations, suppressions,
and workspace index as the CLI. All open buffers participate in one snapshot,
including new files that have never been saved. Include/exclude patterns and
Git ignore files still apply. Ignored buffers remain open but supply no facts.
Closing a new buffer removes it from link resolution; closing an existing one
restores its saved content.

Analysis covers the included workspace so incoming links and other cross-file
findings refresh when a dependency changes. Findings may be published for closed
files. Empty diagnostic arrays clear previous results when a finding disappears,
a path becomes excluded, or a new buffer closes. Open documents carry versions
when the client supports them. Clients also opt into rule-documentation links
and related information; related ranges require a loaded source.

Diagnostics have warning severity, rule codes, source `seiso`, and the existing
repair guidance. Stable rules run by default. Preview selection remains opt-in
and does not promote rules. Input errors are sent through `window/logMessage`;
partial checks retain supported findings but offer no fixes. Fatal configuration
or synchronization errors clear previously published findings.

## Quick fixes

Quick fixes are advertised only to clients supporting code-action literals and
`WorkspaceEdit.documentChanges`. Only open documents receive fixes, and every
edit names their current version. The client applies the edit; the server never
writes source files. Source changes after a response must cause the client to
reject its old version.

Each request rechecks the workspace, since suppression completion can depend on
another document or changed configuration. Only safe fixes from complete checks
are returned. The engine validates edit boundaries and overlaps. Equivalent
fixes are deduplicated, and requested action kinds and ranges filter the result.
Individual quick fixes are supported; fix-all and unsafe rewrites are not.

## Execution limits

The server processes messages and whole-workspace analysis synchronously.
It reuses the content-addressed parse cache unless `--no-cache` is supplied,
but does not maintain an incremental dependency graph or cancel analysis already
in progress. Large repositories and expensive preview rules can delay responses.
Use a client debounce interval and narrow rule selection where needed.

Transport headers are limited to 8 KiB and message bodies to 16 MiB.
Malformed JSON with valid framing receives a parse error; invalid framing ends
the connection. Stdout contains only framed JSON-RPC. Exiting after shutdown
returns success; exiting early or losing stdin before shutdown returns failure.
The server provides no pull diagnostics, hover, completion, formatting, or
standalone editor extension.

[`tests/server.rs`](../../tests/server.rs) exercises protocol replay and a live
stdio subprocess. [`tests/overlays.rs`](../../tests/overlays.rs) compares buffer
analysis with equivalent saved inputs. The
[development guide](../guides/development.md#language-server-latency) gives the
reproducible latency benchmark; its fixture checks cross-file correctness as
well as response time.
