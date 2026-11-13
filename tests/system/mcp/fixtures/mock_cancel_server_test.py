#!/usr/bin/env python3
"""Mock MCP stdio server used by the cancellation tests.

Unlike ``mock_server_test.py`` this server keeps reading stdin while a slow
``tools/call`` is in flight, so it can observe an incoming
``notifications/cancelled``. The slow call deliberately still writes its
late response after being cancelled: the client is required to drop it
instead of routing it to a later request.

Every cancellation-related event is appended as one JSON object per line to
``$MOCK_CANCEL_LOG`` when that variable is set.
"""
import json
import os
import sys
import threading
import time

_write_lock = threading.Lock()
_state_lock = threading.Lock()
_cancelled = set()


def log(event):
    path = os.environ.get("MOCK_CANCEL_LOG")
    if not path:
        return
    with _write_lock:
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(json.dumps(event) + "\n")


def write_message(message):
    line = json.dumps(message)
    with _write_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def slow_call(msg_id, delay_ms):
    """Finish the call even when cancelled, then report that it was late."""
    time.sleep(delay_ms / 1000.0)
    with _state_lock:
        was_cancelled = msg_id in _cancelled
    if was_cancelled:
        log({"event": "late_response_sent", "requestId": msg_id})
    write_message(
        {
            "jsonrpc": "2.0",
            "id": msg_id,
            "result": {
                "content": [{"type": "text", "text": "slow"}],
                "isError": False,
            },
        }
    )


def handle(message):
    if not isinstance(message, dict):
        return
    method = message.get("method")
    msg_id = message.get("id")

    # Notifications have no id and must still be observed: this is where the
    # cancellation notification arrives while `slow_call` is sleeping.
    if msg_id is None:
        if method == "notifications/cancelled":
            params = message.get("params") or {}
            request_id = params.get("requestId")
            if isinstance(request_id, int):
                with _state_lock:
                    _cancelled.add(request_id)
                log({"event": "cancelled", "requestId": request_id})
        return

    if method == "initialize":
        write_message(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {"name": "mock-cancel", "version": "0.0.1"},
                },
            }
        )
    elif method == "tools/list":
        write_message(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echo the provided message",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"message": {"type": "string"}},
                                "required": ["message"],
                            },
                        },
                        {
                            "name": "slow",
                            "description": "Sleep for delay_ms then return",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"delay_ms": {"type": "integer"}},
                                "required": ["delay_ms"],
                            },
                        },
                    ]
                },
            }
        )
    elif method == "tools/call":
        params = message.get("params") or {}
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "echo":
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": "echo:" + str(arguments.get("message", "")),
                            }
                        ],
                        "isError": False,
                    },
                }
            )
        elif name == "slow":
            delay_ms = arguments.get("delay_ms", 3000)
            threading.Thread(
                target=slow_call, args=(msg_id, int(delay_ms)), daemon=True
            ).start()
        else:
            write_message(
                {
                    "jsonrpc": "2.0",
                    "id": msg_id,
                    "error": {"code": -32601, "message": "unknown tool"},
                }
            )
    else:
        write_message(
            {
                "jsonrpc": "2.0",
                "id": msg_id,
                "error": {"code": -32601, "message": "method not found"},
            }
        )


def main():
    while True:
        line = sys.stdin.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        try:
            handle(json.loads(line))
        except json.JSONDecodeError:
            continue


if __name__ == "__main__":
    main()
