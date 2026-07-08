# MCP (Model Context Protocol) Support

xiaoO can expose tools from any [Model Context Protocol](https://modelcontextprotocol.io/) server to the agent loop. Each connected MCP server's tools become first-class xiaoO tools, named `mcp__{server}__{tool}`, and are dispatched to the server via JSON-RPC over stdio or SSE.

This works in **all runtimes**: CLI, TUI, and daemon.

## Configuration

Add an `[mcp]` section with a `[[mcp.servers]]` array entry per server. Place it in `~/.config/xiaoo/config.toml` (CLI/TUI) or the daemon config file.

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
