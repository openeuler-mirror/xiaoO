# Remote TUI Guide

Remote TUI lets the `xiaoo` TUI attach to an `xiaoo-daemon` process **running on the same server**.

- The server runs `xiaoo-daemon` and owns the runtime, LLM provider, tools, hooks, workspace, and operation backend.
- The same server runs `xiaoo` and connects to the daemon with `/remote`.
- Both use the same codebase and binaries; only the startup mode is different.

`/remote` enforces the same-server constraint: every entry into remote mode
compares the daemon's host identity (reported via `/api/v1/health`) against
the TUI's own machine and rejects a daemon on another server at connect time
(see section 3). Remote TUI across machines is not supported — for multi-node
deployment use [xGovernor](https://atomgit.com/openeuler/xGovernor) instead
of `xiaoo-daemon`.

---

## 1. Architecture

```
One server
xiaoo                          xiaoo-daemon
---------                      ----------------
TUI input/rendering   HTTP/SSE  Gateway runtime APIs
/remote commands   ---------->  Agent loop
Interaction prompt  <---------- Tools / hooks / workspace
```

Local TUI remains the default. Remote mode is opt-in:

- `Local`: TUI opens runtimes and runs the agent loop in the local process.
- `Remote`: TUI sends turns to the daemon and renders the daemon's SSE events.

In remote mode, all tool execution happens in the daemon process. The workspace shown in the TUI status bar is marked as remote to avoid confusing it with the TUI's local directory.

"Same server" means the same machine identity: the check compares
`/etc/machine-id` (falling back to hostname). A daemon inside a container has
its own machine-id, so a TUI running on the host is rejected — for
containerized deployments run the TUI inside the same container as the daemon
(see [docker_deploy.md](./docker_deploy.md)).

---

## 2. Start the Daemon

Start the daemon on the same server as the TUI. The default loopback bind is
the common shape for remote TUI:

```bash
xiaoo-daemon --config ~/.config/xiaoo/config.toml
```

A non-loopback bind is still allowed (the daemon also serves non-TUI clients
such as channel webhooks on the same port) but requires a bearer token first —
the daemon refuses to start on a non-loopback address without one:

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
curl http://127.0.0.1:18080/api/v1/health
```

The health response reports the daemon's startup config identity
(`config_path` + `config_sha256`) and its host identity (`host_identity`,
the machine ID of the server it runs on, falling back to hostname). The TUI
compares `config_sha256` against a snapshot of the config content it loaded
at startup, and `host_identity` against its own machine — refusing to enter
remote mode on either mismatch; see section 3.

Disclosure policy for those identity fields: `/api/v1/health` itself is
unauthenticated (docker HEALTHCHECK depends on it), but the identity
fields are returned only when bearer auth is disabled or the request
carries a valid token. With auth disabled the daemon is loopback-bound
(non-loopback binds require auth to start), where any local caller could
read the config file and `/etc/machine-id` itself, so disclosure adds
nothing; on auth-enabled deployments the fields would otherwise leak the
daemon user's home path, a config content fingerprint, and a trackable
host fingerprint to anonymous network callers. The TUI's health probe
sends the bearer token, so the same-config and same-host checks are not
affected; anonymous callers (including `docker HEALTHCHECK`) get `status`
and `version` only.

If bearer auth is configured, protected session/chat routes require:

```bash
-H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN"
```

---

## 3. Start the TUI

Start the TUI normally:

```bash
xiaoo
```

**Same host.** Every entry into remote mode — `/remote <url>`, `auto_connect`,
and activating a recorded remote session from the `/remote` dialog — verifies
that the daemon runs on the same server: the daemon reports `host_identity`
via `/api/v1/health`, the TUI resolves its own machine identity with the same
rules (`/etc/machine-id`, falling back to hostname), and a mismatch refuses
to enter remote mode:

```text
daemon runs on a different host (local=b0e8f34a…, daemon=7f3ac219…); /remote requires xiaoo and xiaoo-daemon on the same server — for multi-node deployment use xGovernor instead (https://atomgit.com/openeuler/xGovernor)
```

Container boundaries count as different hosts (a container has its own
`/etc/machine-id`). Daemons older than host-identity reporting omit the field
and the check is skipped (compatibility).

**Same config.** Both sides must use **the same config file**. The TUI sends local config
overrides (`llm` overrides, `entry.runtime_profile_id`) with every
open/turn request, and the daemon merges them with its own config; when
the two configs differ, those merges fail with errors that do not point
at the config (e.g. `unknown agent role preset`, `missing required API
key environment variable`). To prevent this, every entry into remote
mode — `/remote <url>`, `auto_connect`, and activating a recorded remote
session from the `/remote` dialog — verifies that the daemon's startup
config SHA-256 (reported via `/api/v1/health`) matches the TUI's own
startup snapshot of its config content (re-snapshotted when the TUI
itself rewrites the config, e.g. a provider switch), and refuses to enter
remote mode on mismatch:

```text
config mismatch: TUI /home/B/.config/xiaoo/config.toml (sha256 3f2a1b9c) vs daemon /home/A/.config/xiaoo/config.toml (sha256 9d1c04e2); must use the same config file — both sides hash the config they loaded at startup, so after making both files identical, restart the daemon (and the TUI if its config file changed after it started) and reconnect
```

Copy the daemon-side `config.toml` to the same content on the TUI side before
connecting. The digest comparison covers `config.toml` only (not
`--mcp-config`). Note that both sides hash the config as loaded at
startup: if either config file is edited or re-copied after that side
started, restart that side (the daemon and/or the TUI) so it compares the
current content. Daemons older than config-identity reporting omit the
fields and the check is skipped (compatibility). A health response that
is not the daemon's JSON (an HTML error page from an intermediary proxy,
for example) fails the connect: every daemon version answers this route
with JSON, so anything else means the URL does not reach a daemon.

Connect to the daemon:

```text
/remote http://127.0.0.1:18080
```

If the daemon uses bearer auth, configure the TUI config:

```toml
[tui.remote]
url = "http://127.0.0.1:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = false
```

Then export the same token value before starting the TUI:

```bash
export XIAOO_REMOTE_TOKEN="change-me"
xiaoo
```

When `auto_connect = true`, TUI enters remote backend mode on startup using the
configured URL, running the same health check and same-host / config-match
verification as `/remote <url>` (a mismatch is surfaced as an error message and
the TUI stays local). When `auto_connect = false`, the config only supplies the
bearer token env var and default remote settings; use `/remote <url>` manually.

---

## 4. TUI Commands

| Command | Description |
|---------|-------------|
| `/remote` | Open the remote-session dialog (no argument required); activating a recorded session runs the same health check and same-host / config-match verification as `/remote <base_url>` |
| `/remote <base_url>` | Connect to a gateway daemon on this server, for example `/remote http://127.0.0.1:18080`; refuses to connect when the daemon runs on a different host or its config file differs from the TUI's (see section 3) |
| `/remote status` | Show current backend, remote URL, session-open state, and health result |
| `/remote off` | Close the remote session and switch back to local backend |
| `/remote close` | Close the remote session **on the daemon** (destroys it server-side), unlike `off` which only detaches |
| `/new` | Start a new TUI session; in remote mode this closes the old remote session first |

Bare `/remote` (no argument) opens the session dialog rather than connecting,
and `/remote close` is handled in
`apps/endside/src/input/event_key.rs`.

After `/remote <base_url>` succeeds, new turns go through the daemon. The status bar shows `Remote: <base_url>`.

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
`apps/serverside/src/httpserver/router.rs` (including the cron and channels
routes); the channel webhook route
(`router.rs`) sits outside that group. `/api/v1/health`
(`router.rs`) also bypasses bearer auth — that is what makes the daemon's
HEALTHCHECK work.

Remote TUI directly consumes `open` / `input` / `interaction` / `cancel` /
`close` (the turn lifecycle) **plus** `POST /api/v1/runtimes/detach` and
`POST /api/v1/runtimes/heartbeat`, which implement the attach-lease protocol
that lets a second TUI detect a takeover. `detach` is issued on exit / `/new` /
`/remote off` (`apps/endside/src/gateway_api/remote.rs`, with a bounded
variant); `heartbeat` is issued every 15 s by the app event loop
(`apps/endside/src/gateway_api/remote.rs`, driven from the
`heartbeat_interval.tick()` arm in `apps/endside/src/app/core.rs`). The remaining endpoints
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

- The daemon's config controls the LLM provider, model, workspace, tools, hooks, LSP, and operation backend.
- The TUI's local provider/model config is still used for normal local mode and for TUI bootstrap, but remote turns execute with the daemon's config.
- Use bearer auth for any daemon bound to a non-loopback interface.
- For untrusted networks, prefer an SSH tunnel or TLS-terminating reverse proxy in front of the daemon.
- Remote runtime state is kept in the daemon's in-memory control-plane store. Restarting the daemon loses active remote runtimes in the current implementation.
- **Subagent support.** The daemon binds the same `SubagentControl`
  implementation (`CoreBackedSessionService`) as the local TUI, so
  `spawn_subagent` / `join_subagent` tools work in remote mode out of the
  box. Subagent lane lifecycle (`turn_start` → `tool_call` → `loop_end`)
  and tool-card running state are forwarded via SSE so the
  TUI renders subagent lanes with parity to local mode. Configure
  `[subagent.<id>]` role presets on the daemon; role `prompt` / `tools` /
  `max_turns` are applied at spawn time.

---

## 7. Current Limitations

- `/cancel` is wired through the HTTP/TUI path, but hard cancellation depends on the gateway/core exposing the active loop cancellation token through the session supervisor.
- Remote mode does not sync files from the daemon to the TUI. Tool results and file-change summaries are streamed, but filesystem operations happen only where the daemon runs.
- Remote TUI is not a separate lightweight client package; it is the same `xiaoo` binary running with a remote backend.
- **Single server only.** `/remote` rejects a daemon running on another
  server (host-identity mismatch). Multi-node deployment is out of scope for
  `xiaoo-daemon` — use
  [xGovernor](https://atomgit.com/openeuler/xGovernor) for that topology.

---

## 8. Quick Checklist

1. The daemon has its config and provider credentials.
2. Optionally configure `[http].bearer_token_env` (mandatory for a
   non-loopback bind) and start `xiaoo-daemon --port 18080`.
3. On the same server, the TUI can reach `http://127.0.0.1:18080/api/v1/health`.
4. Export `XIAOO_REMOTE_TOKEN` with the same token if bearer auth is used.
5. Start `xiaoo` on the same server.
6. In TUI, run `/remote http://127.0.0.1:18080`.
7. Send a message and confirm the status bar shows `Remote: http://127.0.0.1:18080`.
