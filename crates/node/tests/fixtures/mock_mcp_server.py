#!/usr/bin/env python3
"""
Minimal MCP stdio server fixture.

Implements just enough of MCP 2025-06-18 for the n3ur0n binding tests:

- initialize           → returns protocolVersion, serverInfo, capabilities.
- tools/list           → advertises one tool named "echo".
- tools/call(echo)     → returns a "text" content item with the JSON-stringified args.

Wire format: one JSON-RPC message per line on stdin/stdout (LSP-style
line-delimited, NOT the Content-Length header form). This matches what
crates/node/src/bindings/mcp_client.rs sends.

Run manually:
    python3 mock_mcp_server.py
    # then paste:
    {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
    {"jsonrpc":"2.0","id":2,"method":"tools/list"}
    {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"x":1}}}
"""

import json
import sys


def reply(msg_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": msg_id, "result": result}) + "\n")
    sys.stdout.flush()


def error(msg_id, code, message):
    sys.stdout.write(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": code, "message": message},
            }
        )
        + "\n"
    )
    sys.stdout.flush()


def handle(msg):
    method = msg.get("method")
    msg_id = msg.get("id")
    params = msg.get("params", {}) or {}

    if method == "initialize":
        reply(
            msg_id,
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mock-mcp", "version": "0.0.1"},
            },
        )
        return

    if method == "notifications/initialized":
        # No reply expected for notifications.
        return

    if method == "tools/list":
        reply(
            msg_id,
            {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Returns its arguments verbatim.",
                        "inputSchema": {"type": "object"},
                    }
                ]
            },
        )
        return

    if method == "tools/call":
        name = params.get("name")
        args = params.get("arguments", {})
        if name != "echo":
            error(msg_id, -32601, f"unknown tool: {name}")
            return
        reply(
            msg_id,
            {
                "content": [{"type": "text", "text": json.dumps(args)}],
                "isError": False,
            },
        )
        return

    error(msg_id, -32601, f"method not found: {method}")


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError as e:
            sys.stderr.write(f"parse error: {e}; raw: {line!r}\n")
            continue
        try:
            handle(msg)
        except Exception as e:  # noqa: BLE001
            sys.stderr.write(f"handler error: {e}\n")
            mid = msg.get("id")
            if mid is not None:
                error(mid, -32603, str(e))


if __name__ == "__main__":
    main()
