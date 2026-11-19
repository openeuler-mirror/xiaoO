# ACP sessions

`xiaoo acp` is the xiaoO subcommand for Agent Client Protocol v1 over stdio.
The launcher provides the configuration. The default profile supports text
conversations. The launcher can enable client-side workspace tools.

## Start the agent

Install from the workspace with `cargo install --locked --path apps/endside`.
Configure the client to launch `xiaoo acp --config /path/to/acp.toml`.
During a session, stdout carries only newline-delimited ACP JSON-RPC messages.

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

## Responsibility boundary

`xiaoo acp` provides the agent side of ACP. Each stdio connection has its own
process. Sessions live in that process's memory. When the selected profile
supports workspace tools, the agent delegates commands and file operations to
the client. When portable conversation state is available, the client stores
exports and supplies them for restoration.

The driving control plane owns execution environments, persistence, agent
process lifecycle, snapshots and branching, cleanup, and scheduling.

## ACP compatibility

Call `initialize` before session methods and proceed if the client supports the
returned `protocolVersion: 1`. Agent capabilities advertise the
`org.openeuler.xiaoo/acp` extension with version 1 and mode `controlled-text` or
`controlled-coding`.

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

Both profiles reject nonempty MCP server lists, including the stdio MCP
configuration that general ACP v1 agents support. Custom system prompts, extra
workspace roots, and persistent session loading are unsupported.

Prompt responses carry `_meta["org.openeuler.xiaoo/usage"]` with `version: 1`,
`inputTokens`, `outputTokens` and `status`. Counts are the provider-reported
totals for the prompt, including reasoning in completion counts where the
provider includes it. `complete` indicates observed counts for completed
requests, `partial` indicates an incomplete observation and `unavailable`
uses null counts. When a prompt fails, usage is attached to the JSON-RPC
error as `error.data.usage`.

## Client workspace tools

Omit the `[delegation]` table or use `permission = "deny"` for text mode and
an empty tool registry. For coding mode, configure the launcher as follows.

```toml
[delegation]
permission = "ask"
exclusive_writer = true
```

`ask` sends `session/request_permission` for each workspace tool call with
`allow_once` and `reject_once` options. A rejection or an unanswered request
fails the call. Use `preauthorized` when the launcher delegates approval
entirely to a trusted client. The client applies its own policy to every
callback in both modes. The in-memory tools do not request workspace access.

| Tool | Required client capabilities |
| --- | --- |
| `bash`, `glob`, `grep` | `terminal` |
| `file_read` | `terminal` and `fs.readTextFile` |
| `file_write`, `file_edit` | `terminal`, `fs.readTextFile`, `fs.writeTextFile`, and launcher setting `exclusive_writer = true` |
| `todo_write`, `ask_user_question` | Coding mode |

`_org.openeuler.xiaoo/tools/list` accepts `{ "sessionId": "..." }` and
returns the session's active tool names and sources as
`{ "tools": [{ "name": "...", "source": "builtin" }] }`.
Tool progress and todo plans use `session/update`. An actual
`ask_user_question` call ends the prompt with `interaction_required`.
Interactive elicitation is unavailable.

The session's absolute `cwd` identifies a logical root in the client's Linux
workspace. The agent resolves paths lexically and rejects traversal above
that root. The client owns real-path and symlink validation, filesystem and
process isolation, per-command permissions, and terminal environment policy.
The agent does not inherit its own environment into client terminals.
Workspace calls fail if the corresponding client method is unavailable or
denied. Workspace and home-directory tool discovery are disabled.

Files use the client's UTF-8 text methods with a 1 MiB content limit. Terminal
callbacks run Bash and Linux filesystem utilities for commands, stat, directory
creation and search. Glob filtering uses GNU grep with PCRE support.
Content search uses ripgrep with a GNU grep fallback when ripgrep
is unavailable. Multiline searches and file-type filters require ripgrep.
Truncated terminal output makes a filesystem query or search fail. Bash
reports the client's combined stream in `stdout` and preserves the client
truncation flag.

Enable `exclusive_writer` only when the client can enforce exclusive writes
from validation through the file callback. The protocol supplies no atomic
overwrite or compare-and-swap operation. A lost write acknowledgement is
never retried automatically.

On cancellation the agent stops admitting operations for that prompt and
kills and releases an in-flight terminal. Terminal cleanup and outstanding
write confirmation have a five-second settlement budget. If the agent cannot
confirm cleanup or the result of a dispatched mutation, the prompt returns
`operation_result_unknown` and the session rejects further prompts. The
client must reconcile its workspace and release resources for the disconnected
session, including any terminal for which the create response was lost.

## Build identity

The release build supplies `XIAOO_RELEASE_VERSION` (SemVer) and
`XIAOO_BUILD_COMMIT` (the full Git SHA) together. `xiaoo --version`,
`xiaoo acp --version`, and `initialize.agentInfo.version` expose the same
version. The `--verbose` version output and the initialize response's
`_meta["org.openeuler.xiaoo/build"].commit` also expose the build commit.
Building without release inputs reports the workspace version and an
unknown commit.

The Rust ACP dependencies are distributed under Apache-2.0. See the
[third-party notice](../License/Third_Party_Open_Source_Software_Notice.md)
for the versions and copyright notices.
