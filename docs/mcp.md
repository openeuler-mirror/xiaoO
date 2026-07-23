# MCP (Model Context Protocol) Support

XiaoO can expose tools from any [Model Context Protocol](https://modelcontextprotocol.io/) server to the agent loop. Each connected MCP server's tools become first-class XiaoO tools, named `mcp__{server}__{tool}`, and are dispatched to the server via JSON-RPC over stdio, legacy SSE, or Streamable HTTP.

This works in **all runtimes**: CLI, TUI, and daemon.

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

- No per-call retry or backoff (a single `timeout_ms` bounds each request).
- No wildcard visibility (`mcp__*__*`); use exact tool names.
- MCP `resources` and `prompts` are not exposed — only `tools`.
- No hot-reload: changes to `[[mcp.servers]]` require restarting xiaoO.

## Troubleshooting

- **Server never connects**: run with `RUST_LOG=mcp=warn` to see spawn/handshake errors. Confirm the `command`/`args` invoke the server manually.
- **Tool not visible to the agent**: check the configured `tools` allowlist includes the exact `mcp__{server}__{tool}` name; a typo produces an "unknown tool name in visibility config" error at startup.
- **Stale connections after config edit**: restart xiaoO; MCP clients are cached for the resolver's lifetime.


---
# Use xiaoO as MCP server
### [mcp_server] - Streamable HTTP MCP Server

The daemon can expose two independent MCP 2025-11-25 Streamable HTTP
endpoints on the same host and port as the runtime API:

| Endpoint | Exposed tool | Capability profile |
|----------|--------------|--------------------|
| `/mcp/chatbot` | `chat` | Only `web_search` and `webfetch` internally |
| `/mcp/agent` | `agent` | Full local Core agent, excluding interactive `ask_user_question` and non-channel `send_file` |

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

Tool inputs are:

```json
{"name":"chat","arguments":{"message":"Hello","session_id":"mcp_chat_..."}}
```

```json
{"name":"agent","arguments":{"message":"Inspect this repository","workspace":"/absolute/existing/directory","session_id":"mcp_agent_..."}}
```

Omit `session_id` to create a session and complete its first turn in the same
call. The result contains both MCP text content and `structuredContent` with
`session_id`, `created`, `reply`, `outcome`, and `usage`. A new agent session
requires an absolute, existing, readable workspace. Later calls may omit it;
if supplied again, its canonical path must match the original binding.
Unknown IDs, IDs from the other endpoint, and workspace conflicts are tool
errors rather than implicit new sessions.

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
