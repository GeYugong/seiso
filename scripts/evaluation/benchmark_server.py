"""Measure stdio LSP refresh latency with real cross-file diagnostic assertions."""

from __future__ import annotations

import argparse
import json
import math
from pathlib import Path
import platform
import queue
import statistics
import subprocess
import tempfile
import threading
import time


def benchmark(binary: Path, documents: int, iterations: int, no_cache: bool) -> dict:
    with tempfile.TemporaryDirectory(prefix="seiso-lsp-") as directory:
        root = Path(directory)
        (root / "seiso.toml").write_text(
            "preview = true\n[lint]\nselect = ['LNK001', 'LNK002']\n",
            encoding="utf-8",
        )
        for number in range(documents - 2):
            (root / f"doc-{number:04}.md").write_text(
                f"---\nkind: reference\n---\n# Document {number}\n\n"
                "[Target](target.md#heading)\n\n"
                + "A paragraph about a stable interface with 中文 and 🦀.\n\n" * 8,
                encoding="utf-8",
            )
        source = "---\nkind: reference\n---\n# Heading\n"
        (root / "target.md").write_text(source, encoding="utf-8")
        (root / "incoming.md").write_text("[Target](target.md#heading)\n", encoding="utf-8")
        target_uri = (root / "target.md").as_uri()
        incoming_uri = (root / "incoming.md").as_uri()
        command = [str(binary), "server"] + (["--no-cache"] if no_cache else [])
        started = time.perf_counter()
        process = subprocess.Popen(command, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        messages: queue.Queue = queue.Queue()

        def reader() -> None:
            try:
                while header := process.stdout.readline():
                    length = int(header.decode("ascii").strip().removeprefix("Content-Length: "))
                    if process.stdout.readline() != b"\r\n":
                        raise ValueError("Invalid LSP framing")
                    messages.put(json.loads(process.stdout.read(length)))
            except Exception as error:
                messages.put(error)
            finally:
                messages.put(EOFError("Server closed stdout"))

        thread = threading.Thread(target=reader, daemon=True)
        thread.start()

        def send(method: str, params: dict | None, request_id: int | None = None) -> None:
            value = {"jsonrpc": "2.0", "method": method, "params": params}
            if request_id is not None:
                value["id"] = request_id
            body = json.dumps(value, ensure_ascii=False).encode("utf-8")
            process.stdin.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body)
            process.stdin.flush()

        def receive() -> dict:
            value = messages.get(timeout=30)
            if isinstance(value, Exception):
                raise value
            return value

        def refresh(version: int, missing: bool) -> None:
            incoming = None
            while True:
                value = receive()
                if value.get("method") != "textDocument/publishDiagnostics":
                    raise AssertionError(value)
                params = value["params"]
                if params["uri"] == incoming_uri:
                    incoming = [item["code"] for item in params["diagnostics"]]
                if params["uri"] == target_uri and params.get("version") == version:
                    # A clean first run need not publish empty diagnostics for unopened files.
                    assert incoming == (["LNK002"] if missing else []) or (version == 1 and incoming is None)
                    assert params["diagnostics"] == []
                    return

        try:
            send("initialize", {
                "rootUri": root.as_uri(),
                "capabilities": {"textDocument": {"publishDiagnostics": {"versionSupport": True}}},
            }, 1)
            assert "result" in receive()
            startup_ms = (time.perf_counter() - started) * 1000
            send("initialized", {})
            started = time.perf_counter()
            send("textDocument/didOpen", {"textDocument": {
                "uri": target_uri, "languageId": "markdown", "version": 1, "text": source,
            }})
            refresh(1, False)
            first_refresh_ms = (time.perf_counter() - started) * 1000
            timings = []
            for index in range(iterations):
                missing = index % 2 == 0
                text = source.replace("Heading", "Renamed") if missing else source
                started = time.perf_counter()
                send("textDocument/didChange", {
                    "textDocument": {"uri": target_uri, "version": index + 2},
                    "contentChanges": [{"text": text}],
                })
                refresh(index + 2, missing)
                timings.append((time.perf_counter() - started) * 1000)
            send("shutdown", None, 2)
            assert receive() == {"jsonrpc": "2.0", "id": 2, "result": None}
            send("exit", None)
            assert process.wait(timeout=5) == 0
            assert (root / "target.md").read_text(encoding="utf-8") == source
            ordered = sorted(timings)
            return {
                "platform": platform.platform(),
                "documents": documents,
                "iterations": iterations,
                "cache": not no_cache,
                "rules": ["LNK001", "LNK002"],
                "startup_ms": round(startup_ms, 3),
                "first_refresh_ms": round(first_refresh_ms, 3),
                "refresh_p50_ms": round(statistics.median(timings), 3),
                "refresh_p95_ms": round(ordered[math.ceil(len(ordered) * 0.95) - 1], 3),
                "refresh_max_ms": round(max(timings), 3),
                "cross_file_refresh_verified": True,
                "source_unchanged": True,
            }
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=5)
            process.stdin.close()
            thread.join(timeout=5)
            process.stdout.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--documents", type=int, default=100)
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--no-cache", action="store_true")
    args = parser.parse_args()
    if args.documents < 2 or args.iterations < 1:
        parser.error("--documents must be at least 2 and --iterations at least 1")
    print(json.dumps(benchmark(args.binary.resolve(), args.documents, args.iterations, args.no_cache), indent=2))


if __name__ == "__main__":
    main()
