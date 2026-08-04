#!/usr/bin/env python3
"""Minimal mock MCP server for xiaoO integration tests.

Speaks JSON-RPC 2.0 over stdio (newline-delimited). Implements:
  - initialize         -> returns serverInfo + capabilities
  - notifications/initialized -> no-op (notification, no response)
  - tools/list         -> returns one tool `echo`
  - tools/call         -> echoes the `message` argument back as text content

Line-buffered: one JSON object per line on stdout, reads one per line on stdin.
"""
import sys
import json


def read_request():
    line = sys.stdin.readline()
    if not line:
        return None
    line = line.strip()
    if not line:
        return None
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return None


def write_response(resp):
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()


def handle(msg):
    if not isinstance(msg, dict):
        return
    method = msg.get("method")
    msg_id = msg.get("id")

    # Notifications have no id; we do not respond.
    if msg_id is None:
        return

    if method == "initialize":
        write_response({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "mock", "version": "0.0.1"},
            },
        })
    elif method == "tools/list":
        write_response({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo the provided message",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "message": {"type": "string"},
                            },
                            "required": ["message"],
                        },
                    }
                ]
            },
        })
    elif method == "tools/call":
        params = msg.get("params") or {}
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name != "echo":
            write_response({
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": "unknown tool"},
            })
            return
        message = arguments.get("message", "")
        write_response({
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "content": [{"type": "text", "text": f"echo:{message}"}],
                "isError": False,
            },
        })
    else:
        write_response({
            "jsonrpc": "2.0",
            "id": msg_id,
            "error": {"code": -32601, "message": "method not found"},
        })


def main():
    while True:
        req = read_request()
        if req is None:
            break
        handle(req)


if __name__ == "__main__":
    main()
