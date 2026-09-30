# Remote TUI Guide

Remote TUI lets one machine run the XiaoO gateway daemon while another machine runs the terminal UI.

- **Machine A** runs `xiaoo-daemon` and owns the runtime, LLM provider, tools, hooks, workspace, and operation backend.
- **Machine B** runs `xiaoo` and connects to Machine A with `/remote`.
- Both machines use the same codebase and binaries; only the startup mode is different.

---

## 1. Architecture

```
Machine B                         Machine A
xiaoo                         xiaoo-daemon
---------                         ----------------
TUI input/rendering   HTTP/SSE    Gateway runtime APIs
/remote commands   ----------->   Agent loop
Interaction prompt  <---------->  Tools / hooks / workspace
```

Local TUI remains the default. Remote mode is opt-in:

- `Local`: TUI opens runtimes and runs the agent loop in the local process.
- `Remote`: TUI sends turns to the daemon and renders the daemon's SSE events.

In remote mode, all tool execution happens on Machine A. The workspace shown in the TUI status bar is marked as remote to avoid confusing it with Machine B's local directory.

---

## 2. Start Machine A

Start the daemon on Machine A. Remote mode binds a non-loopback interface, so a
bearer token is required first — the daemon refuses to start on a non-loopback
address without one:

```toml
# ~/.config/xiaoo/config.toml
[http]
bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN"
```

```bash
export XIAOO_HTTP_BEARER_TOKEN="change-me"
xiaoo-daemon \
  --host 0.0.0.0 \
  --port 18080 \
  --config ~/.config/xiaoo/config.toml
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
xiaoo
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
xiaoo
```

When `auto_connect = true`, TUI enters remote backend mode on startup using the configured URL. When `auto_connect = false`, the config only supplies the bearer token env var and default remote settings; use `/remote <url>` manually.

---

## 4. TUI Commands

| Command | Description |
|---------|-------------|
| `/remote` | Open the remote-session dialog (no argument required) |
| `/remote <base_url>` | Connect to a remote gateway daemon, for example `/remote http://A:18080` |
| `/remote status` | Show current backend, remote URL, session-open state, and health result |
| `/remote off` | Close the remote session and switch back to local backend |
| `/remote close` | Close the remote session **on the daemon** (destroys it server-side), unlike `off` which only detaches |
| `/new` | Start a new TUI session; in remote mode this closes the old remote session first |

Bare `/remote` (no argument) opens the session dialog rather than connecting;
see `apps/endside/src/input/event_key.rs:952`. `/remote close` is handled at
`apps/endside/src/input/event_key.rs:967`.

After `/remote <base_url>` succeeds, new turns go through Machine A's daemon. The status bar shows `Remote: <base_url>`.

---

## 5. Remote Session And Runtime API

Remote TUI uses the daemon's runtime control APIs. The same protected route
group also contains checkpoint APIs for programmatic clients that need branching
runtime state.

| Endpoint | Description |
|----------|-------------|
| `GET /api/v1/runtimes` | Runtime catalog (read-only listing) |
| `GET /api/v1/sandboxes` | Sandbox catalog (read-only listing) |
| `GET /api/v1/runtimes/checkpoints` | Checkpoint catalog (read-only listing) |
| `POST /api/v1/runtimes/open` | Open or resume a runtime using `RuntimeOpenRequest` |
| `POST /api/v1/runtimes/input` | Submit one user input and stream SSE events |
| `POST /api/v1/runtimes/interaction` | Send a user interaction response back to the daemon |
| `POST /api/v1/runtimes/cancel` | Request cancellation of the current turn |
| `POST /api/v1/runtimes/close` | Close the runtime, remove its record, and fire lifecycle hooks |
| `POST /api/v1/runtimes/heartbeat` | Renew this TUI's attach lease (called every 15 s by the TUI event loop) |
| `POST /api/v1/runtimes/detach` | Release this TUI's attach lease without destroying the session or its backend |
| `POST /api/v1/runtimes/checkpoint` | Capture an idle runtime as a checkpoint |
| `POST /api/v1/runtimes/checkpoint/delete-snapshot` | Delete the provider snapshot referenced by a checkpoint |
| `POST /api/v1/runtimes/checkout` | Create a new runtime from a checkpoint |
| `POST /api/v1/runtimes/pause` | Snapshot an idle runtime and release its live backend |
| `POST /api/v1/runtimes/resume` | Restore a paused runtime with the same runtime id |
| `POST /api/v1/runtimes/exec` | Run a shell command inside the runtime's backend |
| `POST /api/v1/runtimes/read-file` | Read a file from the runtime's backend |
| `POST /api/v1/runtimes/write-file` | Write a file inside the runtime's backend |
| `POST /api/v1/runtimes/export` | Export the runtime's conversation/state |
| `GET /api/v1/cron/jobs` | Cron job catalog (read-only listing) |
| `POST /api/v1/cron/run` | Trigger a cron job immediately |
| `GET /api/v1/channels` | Channel catalog (read-only listing) |
| `POST /api/v1/channels/test` | Send a channel test message |
| `POST /api/v1/channels/:channel_id/events` | Channel webhook ingress, keyed by `channel_id` (`feishu` / `telegram`). This one is **not** in the protected group — see note below |

The route list above is drawn from the protected route group at
`apps/serverside/src/httpserver/router.rs:577-614`; the channel webhook route
(`router.rs:620-623`) sits outside that group. `/api/v1/health`
(`router.rs:619`) also bypasses bearer auth — that is what makes the daemon's
HEALTHCHECK work.

Remote TUI directly consumes `open` / `input` / `interaction` / `cancel` /
`close` (the turn lifecycle) **plus** `POST /api/v1/runtimes/detach` and
`POST /api/v1/runtimes/heartbeat`, which implement the attach-lease protocol
that lets a second TUI detect a takeover. `detach` is issued on exit / `/new` /
`/remote off` (`apps/endside/src/gateway_api/remote.rs:413-431`); `heartbeat`
is issued every 15 s by the app event loop
(`apps/endside/src/gateway_api/remote.rs:461-473`, driven from
`apps/endside/src/app/core.rs:425-429`). The remaining endpoints
(`checkpoint`, `checkout`, `pause`, `resume`, `exec`, `read-file`,
`write-file`, the catalogs, cron and channels) are programmatic control-plane
endpoints exposed on the same protected route group for other clients. See
[runtime_checkpoint.md](./runtime_checkpoint.md) for the checkpoint/pause/resume
semantics and `apps/serverside/src/httpserver/router.rs` for the authoritative
route list.

Runtime control payloads use `runtime_id` and `checkpoint_id` as their public
vocabulary.

SSE event types:

| Event | Description |
|-------|-------------|
| `turn_start` | Agent loop turn started; carries `agent_id` (root or subagent) |
| `text_delta` | Assistant text update; includes both incremental `delta` and cumulative `snapshot`; `agent_id` disambiguates root vs. subagent lanes |
| `thinking_delta` | Assistant reasoning text update (mirrors `text_delta` semantics) |
| `tool_result` | Tool execution result summary; carries `agent_id`, `call_id`, `tool_name`, `output_preview`, `is_error`, and `args_preview` |
| `tool_call` | Tool lifecycle transition (`running` / `completed` / `failed` / `denied`). Forwarded by the daemon so the remote TUI can drive the same tool-card state machine as local mode (running spinner, terminal state). `agent_id` routes the update to the root message list or a subagent lane |
| `tool_file_change` | Per-call file change delta precomputed by the daemon so the TUI's session diff panel mirrors the local computation |
| `plan_update` | Plan snapshot parsed by the daemon from the `todo_write` tool's args |
| `subagent_spawn` | Subagent lane metadata parsed by the daemon from the `spawn_subagent` tool's args + output; lets the TUI create the subagent lane without re-parsing the daemon-only `args_preview` |
| `loop_end` | Per-agent loop-end marker. The daemon emits one per `agent_id` (root or subagent) so the TUI can clear `is_running` on the matching subagent lane as its loop terminates, matching local-mode `ChannelLoopEventSink::on_loop_end` semantics. Carries the per-agent `LoopEndSummary` fields (`turn_count`, `total_tokens`, `stop_reason`) so the TUI can render per-agent token usage / stop reason with parity to local mode; older daemons that omit them default to zero / empty via `#[serde(default)]` |
| `interaction_requested` | Daemon asks the TUI to show an interaction prompt |
| `done` | Turn completed; includes token usage and runtime messages |
| `error` | Turn failed |
| `cancelled` | Cancellation acknowledgement |

**Backward / forward compatibility.** The TUI's SSE parser deserializes
each frame into the `RemoteSseEvent` enum, which carries an
`#[serde(other)] Unknown` catch-all variant. Unknown event types (emitted
by a future daemon) are mapped to `Unknown`, logged at `debug` level, and
skipped (not surfaced as a stream error), so a TUI built against this
catalogue keeps working when a future daemon emits additional events — no
hand-maintained string whitelist is needed, and adding a new variant to
`RemoteSseEvent` automatically makes it a known type. New fields on
existing events are added with `#[serde(default)]` so older daemons that
omit them still parse on a newer TUI, and older TUIs that don't know about
a new field silently drop it (serde's default is to allow unknown fields). Daemon authors adding new
SSE events or fields should mirror the snake_case naming of the existing
catalogue and document the additions in this section.

---

## 6. Operational Notes

- Machine A's config controls the LLM provider, model, workspace, tools, hooks, LSP, and operation backend.
- Machine B's local provider/model config is still used for normal local mode and for TUI bootstrap, but remote turns execute with Machine A's daemon config.
- Use bearer auth for any daemon bound to a non-loopback interface.
- For untrusted networks, prefer an SSH tunnel or TLS-terminating reverse proxy in front of the daemon.
- Remote runtime state is kept in the daemon's in-memory control-plane store. Restarting Machine A's daemon loses active remote runtimes in the current implementation.
- **Subagent support.** The daemon binds the same `SubagentControl`
  implementation (`CoreBackedSessionService`) as the local TUI, so
  `spawn_subagent` / `join_subagent` tools work in remote mode out of
  the box. Subagent lane lifecycle (`turn_start` → `tool_call` →
  `loop_end`) and tool-card running state are forwarded via SSE so the
  TUI renders subagent lanes with parity to local mode. Configure
  `[subagent.<id>]` role presets on Machine A; role `prompt` / `tools` /
  `max_turns` are applied at spawn time.

---

## 7. Current Limitations

- `/cancel` is wired through the HTTP/TUI path, but hard cancellation depends on the gateway/core exposing the active loop cancellation token through the session supervisor.
- Remote mode does not sync files from Machine A to Machine B. Tool results and file-change summaries are streamed, but filesystem operations happen only on Machine A.
- Remote TUI is not a separate lightweight client package; it is the same `xiaoo` binary running with a remote backend.

---

## 8. Quick Checklist

1. Machine A has daemon config and provider credentials.
2. Machine A configures `[http].bearer_token_env` and starts
   `xiaoo-daemon --host 0.0.0.0 --port 18080` (a non-loopback bind requires the token).
3. Machine B can reach `http://A:18080/api/v1/health`.
4. Machine B exports `XIAOO_REMOTE_TOKEN` with the same token.
5. Machine B starts `xiaoo`.
6. In TUI, run `/remote http://A:18080`.
7. Send a message and confirm the status bar shows `Remote: http://A:18080`.
