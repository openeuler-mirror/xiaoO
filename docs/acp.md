# ACP text sessions

`xiaoo-acp` is a stdio Agent Client Protocol v1 agent for text conversations. It
uses a configuration provided by the process launcher and keeps sessions in
memory for the lifetime of the connection.

## Start the agent

Install from the workspace with `cargo install --locked --path apps/acp`. Use
`xiaoo-acp --config /path/to/acp.toml` as the ACP client command. Its stdout
carries newline-delimited JSON-RPC messages.

```toml
default_model = "work"

[models.work]
provider = "openai"
model = "your-model"
api_key_env = "XIAOO_MODEL_API_KEY"
# api_base = "https://your-provider.example/v1/chat/completions"
# context_window = 32768
```

Each model profile requires exactly one credential reference. Use
`api_key_env` for a process environment variable or `api_key_file` for an
absolute path to a trusted text file. The launcher controls the configuration,
provider endpoints, and credential references. Clients choose a configured
profile ID and never submit model credentials. By default the agent uses a
32768-token context window without querying a model catalog.

## ACP compatibility

Call `initialize` before session methods and proceed if the client supports the
returned `protocolVersion: 1`. Agent capabilities advertise the
`org.openeuler.xiaoo/acp` extension with mode `controlled-text` and version 1.

| Method | Behavior |
| --- | --- |
| `session/new` | Requires an absolute `cwd` and an empty `mcpServers` array. Use `_meta["org.openeuler.xiaoo/acp"].modelProfile` to select an initial profile. |
| `session/set_config_option` | Set the `model` option to a configured profile ID while the session is idle. Subsequent turns keep the history and use the selected model. |
| `session/prompt` | Accepts text and resource links as context. Resource links contribute their name and URI as text. Agent text and reasoning deltas are sent through `session/update` before the final response. One prompt may be active per session. |
| `session/cancel` | Cancels the current prompt for the given session. A cancelled prompt finishes with `stopReason: "cancelled"`. |
| `session/close` | Cancels any active prompt and releases the session. If the close deadline expires, cleanup continues and new prompts are rejected during cleanup. End of stdin closes the connection and its sessions. |

If the provider restarts or replaces text already queued for delivery, the
prompt returns an error. Clients should discard the failed prompt's partial
updates.

The controlled profile has an empty tool registry. All nonempty MCP server
lists are rejected, including the stdio MCP configuration that general ACP v1
agents support. File and terminal callbacks, custom system prompts, extra
workspace roots, and persistent session loading are unsupported.

Prompt responses carry `_meta["org.openeuler.xiaoo/usage"]` with `version: 1`,
`inputTokens`, `outputTokens` and `status`. Counts are the provider-reported
totals for the prompt, including reasoning in completion counts where the
provider includes it. `complete` indicates observed counts for completed
requests, `partial` indicates an incomplete observation and `unavailable`
uses null counts. When a prompt fails, usage is attached to the JSON-RPC
error as `error.data.usage`.

## Build identity

The release build supplies `XIAOO_RELEASE_VERSION` (SemVer) and
`XIAOO_BUILD_COMMIT` (the full Git SHA) together. `--version` and
`initialize.agentInfo.version` expose the same version.
`--version --verbose` and the initialize response's
`_meta["org.openeuler.xiaoo/build"].commit` also expose the build commit.
Building without release inputs reports the workspace version with a
`-dev` suffix and an unknown commit.

The Rust ACP dependencies are distributed under Apache-2.0. See the
[third-party notice](../License/Third_Party_Open_Source_Software_Notice.md)
for the versions and copyright notices.
