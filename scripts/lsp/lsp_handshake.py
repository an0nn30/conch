#!/usr/bin/env python3
"""Drive one LSP `initialize`/`shutdown`/`exit` handshake over stdio.

Used by ``scripts/lsp/smoke-macos-arm64.sh`` to prove a packaged server inside a
signed app bundle actually answers the protocol, not merely that its file
exists. Only the framing and the shape of the initialize result are asserted;
capability contents belong to the Rust adapter tests.

Usage:
    lsp_handshake.py --root <workspace-dir> [--timeout SECONDS] -- <program> [args...]
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
from typing import Dict, List, Optional
from urllib.request import pathname2url


def frame(payload: Dict[str, object]) -> bytes:
    body = json.dumps(payload).encode("utf-8")
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)


def read_message(stream) -> Optional[Dict[str, object]]:
    """Read one Content-Length framed message, or None at end of stream."""
    headers: Dict[str, str] = {}
    while True:
        line = stream.readline()
        if not line:
            return None
        line = line.decode("utf-8", "replace").strip()
        if line == "":
            break
        name, separator, value = line.partition(":")
        if not separator:
            raise ValueError(f"malformed LSP header line: {line!r}")
        headers[name.strip().lower()] = value.strip()
    if "content-length" not in headers:
        raise ValueError(f"LSP message has no Content-Length header: {headers!r}")
    length = int(headers["content-length"])
    body = stream.read(length)
    if body is None or len(body) != length:
        raise ValueError("LSP message body truncated")
    return json.loads(body.decode("utf-8"))


def await_response(process: subprocess.Popen, request_id: int) -> Dict[str, object]:
    """Return the response with ``request_id``, skipping notifications."""
    while True:
        message = read_message(process.stdout)
        if message is None:
            raise SystemExit("server closed stdout before responding to initialize")
        if message.get("id") == request_id and (
            "result" in message or "error" in message
        ):
            return message


def handshake(program: List[str], root: str, timeout: float) -> int:
    root = os.path.abspath(root)
    initialize = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": os.getpid(),
            "clientInfo": {"name": "termlab-lsp-smoke", "version": "1"},
            "rootUri": "file://" + pathname2url(root),
            "workspaceFolders": [
                {"uri": "file://" + pathname2url(root), "name": os.path.basename(root)}
            ],
            "capabilities": {
                "textDocument": {"completion": {"completionItem": {"snippetSupport": False}}},
                "workspace": {"configuration": True, "workspaceFolders": True},
            },
        },
    }

    process = subprocess.Popen(
        program,
        cwd=root,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )

    # Drain stderr continuously. A server that logs more than the ~64KB pipe
    # buffer before answering would otherwise block writing to a pipe nobody is
    # reading, and the handshake would look like a timeout rather than a chatty
    # server. Draining also means the timeout path has something to report.
    stderr_chunks: List[bytes] = []

    def drain_stderr() -> None:
        try:
            for line in iter(process.stderr.readline, b""):
                stderr_chunks.append(line)
        except (OSError, ValueError):
            pass

    stderr_reader = threading.Thread(target=drain_stderr, daemon=True)
    stderr_reader.start()

    def captured_stderr() -> str:
        return b"".join(stderr_chunks).decode("utf-8", "replace").strip()

    result: Dict[str, object] = {}
    failure: List[str] = []

    def exchange() -> None:
        try:
            process.stdin.write(frame(initialize))
            process.stdin.flush()
            response = await_response(process, 1)
            if "error" in response:
                failure.append(f"initialize returned an error: {response['error']}")
                return
            body = response.get("result")
            if not isinstance(body, dict) or "capabilities" not in body:
                failure.append(f"initialize result has no capabilities: {body!r}")
                return
            result.update(body)
            process.stdin.write(frame({"jsonrpc": "2.0", "method": "initialized", "params": {}}))
            process.stdin.write(frame({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}))
            process.stdin.flush()
            await_response(process, 2)
            process.stdin.write(frame({"jsonrpc": "2.0", "method": "exit"}))
            process.stdin.flush()
        except Exception as error:  # noqa: BLE001 - reported, not swallowed
            failure.append(f"{type(error).__name__}: {error}")

    worker = threading.Thread(target=exchange, daemon=True)
    worker.start()
    worker.join(timeout)

    if worker.is_alive():
        process.kill()
        stderr_reader.join(2)
        print(f"smoke: handshake timed out after {timeout:.0f}s", file=sys.stderr)
        stderr = captured_stderr()
        if stderr:
            print(f"smoke: server stderr: {stderr[:2000]}", file=sys.stderr)
        else:
            print("smoke: the server wrote nothing to stderr", file=sys.stderr)
        return 1

    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
    stderr_reader.join(2)

    if failure:
        print(f"smoke: {failure[0]}", file=sys.stderr)
        stderr = captured_stderr()
        if stderr:
            print(f"smoke: server stderr: {stderr[:2000]}", file=sys.stderr)
        return 1

    server = result.get("serverInfo") or {}
    name = server.get("name", "server") if isinstance(server, dict) else "server"
    version = server.get("version", "") if isinstance(server, dict) else ""
    print(f"smoke: initialize answered by {name} {version}".rstrip())
    return 0


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True)
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("program", nargs=argparse.REMAINDER)
    arguments = parser.parse_args(argv)
    program = [item for item in arguments.program if item != "--"]
    if not program:
        parser.error("no server program given")
    return handshake(program, arguments.root, arguments.timeout)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
