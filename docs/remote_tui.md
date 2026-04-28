# Remote TUI Guide

Remote TUI lets one machine run the XiaoO gateway daemon while another machine runs the terminal UI.

- **Machine A** runs `xiaoo-app daemon` and owns the runtime, LLM provider, tools, hooks, workspace, and operation backend.
- **Machine B** runs `xiaoo-tui` and connects to Machine A with `/remote`.
- Both machines use the same codebase and binaries; only the startup mode is different.

---

## 1. Architecture

```
Machine B                         Machine A
xiaoo-tui                         xiaoo-app daemon
---------                         ----------------
TUI input/rendering   HTTP/SSE    Gateway session APIs
/remote commands   ----------->   Agent loop
Interaction prompt  <---------->  Tools / hooks / workspace
```

Local TUI remains the default. Remote mode is opt-in:

- `Local`: TUI opens sessions and runs the agent loop in the local process.
- `Remote`: TUI sends turns to the daemon and renders the daemon's SSE events.

In remote mode, all tool execution happens on Machine A. The workspace shown in the TUI status bar is marked as remote to avoid confusing it with Machine B's local directory.

---

## 2. Start Machine A

Start the daemon on Machine A:

```bash
xiaoo-app daemon \
  --host 0.0.0.0 \
  --port 18080 \
  --config ~/.config/xiaoo/config.toml
```

Recommended daemon auth configuration:

```toml
[http]
bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN"
```

Then export the token before starting the daemon:

```bash
export XIAOO_HTTP_BEARER_TOKEN="change-me"
xiaoo-app daemon --host 0.0.0.0 --port 18080
```

Health check:

```bash
curl http://A:18080/api/v1/health
```

If bearer auth is configured, protected session/chat routes require:

```bash
-H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN"
```

---

## 3. Start Machine B

Start the TUI normally:

```bash
xiaoo-tui
```

Connect to Machine A:

```text
/remote http://A:18080
```

If Machine A uses bearer auth, configure Machine B's TUI config:

```toml
[tui.remote]
url = "http://A:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = false
```

Then export the same token value on Machine B:

```bash
export XIAOO_REMOTE_TOKEN="change-me"
xiaoo-tui
```

When `auto_connect = true`, TUI enters remote backend mode on startup using the configured URL. When `auto_connect = false`, the config only supplies the bearer token env var and default remote settings; use `/remote <url>` manually.

---

## 4. TUI Commands

| Command | Description |
|---------|-------------|
| `/remote <base_url>` | Connect to a remote gateway daemon, for example `/remote http://A:18080` |
| `/remote status` | Show current backend, remote URL, session-open state, and health result |
| `/remote off` | Close the remote session and switch back to local backend |
| `/new` | Start a new TUI session; in remote mode this closes the old remote session first |

After `/remote <base_url>` succeeds, new turns go through Machine A's daemon. The status bar shows `Remote: <base_url>`.

---

## 5. Remote Session API

Remote TUI uses the daemon's session APIs, not the older channel-style `/api/v1/chat` endpoint.

| Endpoint | Description |
|----------|-------------|
| `POST /api/v1/sessions/open` | Open or resume a gateway session using `SessionOpenRequest` |
| `POST /api/v1/sessions/{session_id}/turn/stream` | Run one turn and stream SSE events |
| `POST /api/v1/sessions/{session_id}/interaction` | Send a user interaction response back to the daemon |
| `POST /api/v1/sessions/{session_id}/cancel` | Request cancellation of the current turn |
| `POST /api/v1/sessions/{session_id}/close` | Close the session and fire lifecycle hooks |

SSE event types:

| Event | Description |
|-------|-------------|
| `turn_start` | Agent loop turn started |
| `text_delta` | Assistant text update; includes both incremental `delta` and cumulative `snapshot` |
| `tool_result` | Tool execution result summary |
| `interaction_requested` | Daemon asks the TUI to show an interaction prompt |
| `done` | Turn completed; includes token usage and session messages |
| `error` | Turn failed |
| `cancelled` | Cancellation acknowledgement |

---

## 6. Operational Notes

- Machine A's config controls the LLM provider, model, workspace, tools, hooks, LSP, and operation backend.
- Machine B's local provider/model config is still used for normal local mode and for TUI bootstrap, but remote turns execute with Machine A's daemon config.
- Use bearer auth for any daemon bound to a non-loopback interface.
- For untrusted networks, prefer an SSH tunnel or TLS-terminating reverse proxy in front of the daemon.
- Remote session state is kept in the daemon's in-memory session store. Restarting Machine A's daemon loses active remote sessions in the current implementation.

---

## 7. Current Limitations

- `/cancel` is wired through the HTTP/TUI path, but hard cancellation depends on the gateway/core exposing the active loop cancellation token through the session supervisor.
- Remote mode does not sync files from Machine A to Machine B. Tool results and file-change summaries are streamed, but filesystem operations happen only on Machine A.
- Remote TUI is not a separate lightweight client package; it is the same `xiaoo-tui` binary running with a remote backend.

---

## 8. Quick Checklist

1. Machine A has daemon config and provider credentials.
2. Machine A starts `xiaoo-app daemon --host 0.0.0.0 --port 18080`.
3. Machine B can reach `http://A:18080/api/v1/health`.
4. If auth is enabled, Machine B exports `XIAOO_REMOTE_TOKEN`.
5. Machine B starts `xiaoo-tui`.
6. In TUI, run `/remote http://A:18080`.
7. Send a message and confirm the status bar shows `Remote: http://A:18080`.
