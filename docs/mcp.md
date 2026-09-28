# MCP (Model Context Protocol) Support

XiaoO can expose tools from any [Model Context Protocol](https://modelcontextprotocol.io/) server to the agent loop. Each connected MCP server's tools become first-class XiaoO tools, named `mcp__{server}__{tool}`, and are dispatched to the server via JSON-RPC over stdio, legacy SSE, or Streamable HTTP.

This works in **all runtimes**: CLI, TUI, and daemon.

## Opt-in long-term memory automation

Set `[memory_automation] enabled = true` and select an existing `[[mcp.servers]]`
entry with `server = "ram-a"`. The selected server must provide both
`memory_search` and `memory_ingest`. Recall is appended as bounded
`<untrusted_long_term_memory>` system context; it never modifies the user
message. Ingest is queued durably and MCP failures only produce degraded logs,
never a failed completed turn. Use `allowed_agent_roles` to limit automation.

```toml
[memory_automation]
enabled = true
server = "ram-a"
recall_top_k = 5
recall_token_budget = 512
context_messages = 4
queue_path = "memory-queue.jsonl"
queue_capacity = 256
max_retries = 5
retry_backoff_ms = 250
allowed_agent_roles = ["main"]
```

`Mem OK` in the TUI reports RAM-A connection and recent-operation health; it
does not override `allowed_agent_roles`. A turn whose active role is not
allowlisted deliberately skips both recall and ingest.

## Configuration

Add an `[mcp]` section with a `[[mcp.servers]]` array entry per server. Place it in `~/.config/xiaoo/config.toml` (CLI/TUI) or the daemon config file.

XiaoO also imports the standard `mcpServers` object from JSON. The lookup order
is deterministic:

1. `--mcp-config <path>`
2. `XIAOO_MCP_CONFIG`
3. `.mcp.json` in the current workspace
4. `~/.config/xiaoo/mcp.json`

An explicitly selected file must exist, and any selected file must parse and
validate successfully. Invalid JSON is a startup error rather than an empty
configuration. JSON entries are runtime-only: TUI configuration saves never
copy them into `config.toml`. A server name present in both TOML and JSON is a
startup error that identifies both source files; entries are never silently
overwritten.

### Standard `.mcp.json` Streamable HTTP server

The key under `mcpServers` becomes the server name:

```json
{
  "mcpServers": {
    "ram-a": {
      "transport": "streamable_http",
      "url": "http://127.0.0.1:18081/mcp",
      "bearer_token_env": "RAM_A_TOKEN",
      "agent_id": "xiaoo",
      "headers": {
        "X-XiaoO-Client": "ram-a"
      },
      "timeout_ms": 30000
    }
  }
}
```

`transport` uses the exact string `streamable_http`. Unknown fields, invalid
HTTP(S) URLs, zero timeouts, malformed headers, and secret-bearing headers
such as `Authorization` are rejected. `bearer_token_env` stores only the name
of an environment variable; put the token in that environment variable, never
in JSON. Fixed `headers` are intended only for non-sensitive routing or client
metadata.

### RAM-A Streamable HTTP memory server

For RAM-A, point xiaoO at the server's `/mcp` endpoint and read the bearer
token from the environment:

```json
{
  "mcpServers": {
    "ram-a": {
      "transport": "streamable_http",
      "url": "http://127.0.0.1:18081/mcp",
      "bearer_token_env": "RAM_A_XIAOO_TOKEN",
      "agent_id": "xiaoo",
      "timeout_ms": 30000
    }
  }
}
```

Then enable automatic memory explicitly in `config.toml`:

```toml
[memory_automation]
enabled = true
server = "ram-a"
recall_top_k = 5
recall_token_budget = 512
context_messages = 4
queue_path = "memory-automation-queue.jsonl"
queue_capacity = 256
max_retries = 5
retry_backoff_ms = 250
allowed_agent_roles = ["main"]
```

`Mem OK` in the TUI reports RAM-A connection and recent-operation health; it
does not override `allowed_agent_roles`. A turn whose active role is not
allowlisted deliberately skips both recall and ingest.

RAM-A expects MCP protocol version `2025-11-25` and bearer auth on every MCP
request. If `agent_id` is configured, xiaoO sends `X-Agent-ID`; it must match
the RAM-A token binding. Do not put transport-managed headers such as
`Origin`, `Authorization`, `X-Agent-ID`, `mcp-session-id`, or
`mcp-protocol-version` in `.mcp.json` `headers`; xiaoO rejects them during
config validation. A local non-browser xiaoO process normally omits `Origin`.
Keep the RAM-A service on localhost or behind a TLS reverse proxy, and use the
RAM-A deployment guide for SQLite, provider, and single-instance limits.

### stdio server (local subprocess)

```toml
[[mcp.servers]]
name = "filesystem"                    # logical name; tools become mcp__filesystem__<tool>
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
env = { }                              # optional extra env vars for the child
enabled = true                         # optional, default true
timeout_ms = 30000                     # handshake + per-request timeout
```

### SSE server (remote HTTP+SSE)

```toml
[[mcp.servers]]
name = "remote-tools"
transport = "sse"
url = "http://localhost:8080"
timeout_ms = 30000
```

### Disabling a server without removing it

```toml
[[mcp.servers]]
name = "experimental"
command = "node"
args = ["./exp-server.js"]
enabled = false
```

### Declaring effect profile (parallel execution)

The MCP protocol does not expose whether a tool is read-only or has side
effects, so xiaoO defaults to the most conservative assumption: any batch
containing an MCP tool is executed sequentially. If you know a server's tools
are read-only, declare an `[mcp.servers.effect]` section so they can run in
parallel with other parallel-safe tools:

```toml
[[mcp.servers]]
name = "lookup"
transport = "stdio"
command = "./lookup-server"

[mcp.servers.effect]
reads_filesystem = true      # reads the local fs
writes_filesystem = false    # does not write the fs
network_access = false       # no network
side_effects = false         # no other side effects
```

All four fields default to `true`. A tool is parallel-safe only when
`writes_filesystem` and `side_effects` are both `false` **and** at least one of
`reads_filesystem` or `network_access` is `true`.

## How tools are surfaced

At session start, xiaoO's runtime resolver:

1. Spawns/connects each enabled MCP server.
2. Performs the MCP `initialize` handshake and sends `notifications/initialized`.
3. Calls `tools/list` (following pagination cursors) to enumerate tools.
4. Registers every returned tool as `mcp__{server_name}__{tool_name}`.

Unreachable servers are logged at `warn` level and skipped — they never block agent startup.

## Visibility per agent / subagent

MCP tools participate in the same `[subagent.<role>.tools]` and `[agent.<role>.tools]` visibility mechanism as builtins. Reference them by their full namespaced name:

```toml
[subagent.researcher]
description = "Research specialist"
max_turns = 8

[subagent.researcher.tools]
"mcp__filesystem__read_file" = true
"mcp__filesystem__write_file" = false
```

If no `tools` map is configured for a role, all MCP tools are visible by default (same as builtins).

## Tool semantics

- **Input schema**: the MCP server's JSON Schema is passed through to the LLM unchanged — the model sees exactly what the server declares.
- **Output**: MCP `content` blocks are flattened to a string. Text blocks are concatenated; image and resource blocks are summarised as placeholders (e.g. `[image mime=image/png bytes=2048]`).
- **Errors**: if the server returns `is_error: true`, the tool result is marked as an error for the agent; JSON-RPC-level errors surface as a failed tool execution.

## Lifecycle

- Connections are lazily established on the first session resolve and cached for the lifetime of the resolver — subsequent sessions reuse the live connections.
- stdio child processes are killed when the xiaoO process exits (`kill_on_drop`). A graceful `shutdown` is best-effort.
- SSE connections are dropped when the resolver is dropped.

## Limitations (current)

- Retries are deliberately narrow. Tool calls are **not** retried or
  backed off: a single `timeout_ms` bounds each request. The one exception is
  `initialize`, which is retried once when the server answers 429 with
  `Retry-After` (`crates/mcp/src/transport/streamable_http.rs:720-757`); the
  header is parsed at `:964-970`. Streamable HTTP SSE streams also resume after
  a drop, honouring the server's `retry` field as the delay before reconnecting
  with `Last-Event-ID` (`crates/mcp/src/transport/streamable_http.rs:617-660`).
  Arbitrary tool calls are excluded from the 429 retry because the server may
  have already performed a side effect.
- No wildcard visibility (`mcp__*__*`); use exact tool names.
- MCP `resources` and `prompts` are not exposed — only `tools`.
- No hot-reload: changes to `[[mcp.servers]]` require restarting xiaoO.

## Troubleshooting

- **Server never connects**: run with `RUST_LOG=mcp=warn` to see spawn/handshake errors. Confirm the `command`/`args` invoke the server manually.
- **Tool not visible to the agent**: check the configured `tools` allowlist includes the exact `mcp__{server}__{tool}` name; a typo produces an "unknown tool name in visibility config" error at startup.
- **Stale connections after config edit**: restart xiaoO; MCP clients are cached for the resolver's lifetime.

### Diagnostics

Prefer the built-in reports over manual reproduction:

- `xiaoo-daemon config mcp` prints the resolved MCP client catalog as JSON,
  including the JSON-config path resolved from `--mcp-config`/`~/.xiaoo`
  (`apps/serverside/src/main.rs:265-287`; catalog assembly in
  `apps/serverside/src/mcp_management.rs:15`).
- `xiaoo-daemon config mcp-server` prints the `[mcp_server]` preflight report —
  validation of the endpoint tokens, workspace, origins, and `agent_role`
  references (`apps/serverside/src/main.rs:326-334`;
  `apps/serverside/src/mcp_server_management.rs:53-193`).

Both are reached via the `config` subcommand dispatch
(`apps/serverside/src/main.rs:1088-1089`). Shared resolution helpers for the
JSON config location live in `apps/shared/src/mcp_support.rs:22-115`.


---
# Use xiaoO as MCP server
### [mcp_server] - Streamable HTTP MCP Server

The daemon can expose two independent MCP 2025-11-25 Streamable HTTP
endpoints on the same host and port as the runtime API:

| Endpoint | Exposed tools | Capability profile |
|----------|---------------|--------------------|
| `/mcp/chatbot` | `chat` | Only `web_search` and `webfetch` internally |
| `/mcp/agent` | `agent`, `agent_status` | Full local Core agent, or a fixed configured agent role, excluding interactive `ask_user_question` and non-channel `send_file` |

`/mcp/agent` exposes two tools (`apps/serverside/src/mcp_server.rs:598-629`):

- `agent` starts an operation and returns immediately.
- `agent_status` polls an operation previously returned by `agent`. It takes
  `operation_id` and is the only way to observe completion; `agent_status`
  rejects an empty `operation_id` as a tool error
  (`apps/serverside/src/mcp_server.rs:621-628`).

```toml
[mcp_server]
enabled = true
idle_timeout_secs = 600
reaper_interval_secs = 30
# Browser requests carrying Origin are rejected when this is empty.
allowed_origins = []

[mcp_server.chatbot]
bearer_token_env = "XIAOO_MCP_CHATBOT_TOKEN"
workspace = "~/.xiaoo/mcp-chatbot-empty"

[mcp_server.agent]
bearer_token_env = "XIAOO_MCP_AGENT_TOKEN"
# Optional: bind every new /mcp/agent session to an [agent.<role>] preset.
agent_role = "xuanyuan"

# MCP agent mode requires the local backend. Omitting this section also
# selects the implicit local backend.
[server.operation_backend]
kind = "local"
```

Set both secrets before starting the daemon. They must be non-empty and
different:

```bash
export XIAOO_MCP_CHATBOT_TOKEN='replace-with-chatbot-token'
export XIAOO_MCP_AGENT_TOKEN='replace-with-agent-token'
xiaoo-daemon --host 127.0.0.1 --port 18080
```

Every MCP `GET`, `POST`, and `DELETE` request requires the endpoint-specific
`Authorization: Bearer ...` header. The normal `[http]` bearer token does not
grant access to either MCP endpoint. `[http.rate_limit]` also applies to MCP
requests.

The chatbot workspace is created at startup if necessary and must be empty.
The daemon refuses to start rather than deleting files from a non-empty
directory. It is only a fixed runtime working directory: the chatbot has no
file-read, file-search, file-write, or shell tools. Skills, plugins, upstream
MCP tools, hooks, role switching, planning, subagents, and LSP are also
disabled for this profile.

`mcp_server.agent.agent_role` is optional. When set, it must name an existing
`[agent.<role>]` section. New `/mcp/agent` sessions use that role's prompt,
turn limit, and tool visibility policy; clients cannot override the role in a
tool call. Existing sessions keep the role they were created with, so start a
new MCP agent session after changing this setting. When omitted, `/mcp/agent`
continues to use the role-neutral Core prompt.

Tool inputs are:

```json
{"name":"chat","arguments":{"message":"Hello","session_id":"mcp_chat_..."}}
```

```json
{"name":"agent","arguments":{"message":"Inspect this repository","workspace":"/absolute/existing/directory","session_id":"mcp_agent_..."}}
```

```json
{"name":"agent_status","arguments":{"operation_id":"op_..."}}
```

For `chat`, omit `session_id` to create a session and complete its first turn in
the same call; the result contains both MCP text content and `structuredContent`
with `session_id`, `created`, `reply`, `outcome`, and `usage`
(`apps/serverside/src/mcp_server.rs:774-810`).

For `agent`, the call is **asynchronous**. It starts the operation and returns
immediately: `structuredContent` is `AgentOperationOutput{operation_id,
session_id, created, state}` where `state` is a tagged union
(`apps/serverside/src/mcp_server.rs:168-191`):

- `state: "running"` — carries `poll_after_ms` and a `snapshot` of the latest
  root-agent turn. Do not call again before `poll_after_ms` elapses; the server
  enforces that interval by making an early request wait until the next poll is
  due (`apps/serverside/src/mcp_server.rs:610-615`).
- `state: "done"` — carries `reply` (the complete result), `outcome`, `usage`,
  and an optional `error`.

So for `agent` the first turn is *not* completed in the same call: an omitted
`session_id` creates the session and the operation starts running, and you must
poll `agent_status` with the returned `operation_id` until `state` is `done`
before starting another operation in that session. Calling `agent` again for a
session with a live operation returns a busy tool error naming the
`operation_id` to poll (`apps/serverside/src/mcp_server.rs:915-926`).

A new agent session requires an absolute, existing, readable workspace. Later
calls may omit it; if supplied again, its canonical path must match the original
binding. Unknown IDs, IDs from the other endpoint, and workspace conflicts are
tool errors rather than implicit new sessions.

After `idle_timeout_secs` with no active or queued turn, the daemon releases
the local runtime and keeps the in-memory conversation record. A later call
with the same ID rebuilds the local backend and continues the context. The
record is process-local: restarting the daemon loses MCP application sessions.
MCP transport-session `DELETE` closes only the protocol connection and does
not delete the xiaoO application session.

The agent token grants the effective permissions of the Unix account running
the daemon. With unrestricted local isolation, full-agent tools can access
host paths outside the selected workspace; use OS isolation and protect this
token accordingly.
