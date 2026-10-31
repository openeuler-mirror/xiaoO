# CLI Configuration Guide

> **Note**: This document focuses on CLI (`xiaoo --cli run`) configuration options and usage.
>
> For **common configuration items** (llm, subagent, skills, compact, trace, hooker, etc.), please refer to [Configuration File Guide](./config_file_guide.md).

---

## CLI Configuration Overview

CLI is the simplest running mode, supporting all common configuration items, but **does not support** the following specialized configurations:

| Configuration | CLI Support | Description |
|--------|---------|------|
| `[llm]` | ✅ | LLM provider configuration (`profiles`/`active_profile`/`context_window` are daemon-only) |
| `[subagent]` | ✅ | Predefined subagent roles ⭐ |
| `[skills]` | ✅ | Skills configuration (dirs/allow_scripts/disabled only) |
| `[compact]` | ✅ | Context compression configuration |
| `[trace]` | ✅ | Tracing configuration |
| `[hooker]` | ✅ | Hooker configuration |
| `[operation_backend]` | ✅ | Operation backend configuration |
| `[vault]` | ✅ | Encrypted secrets storage (see vault_secrets_design.md; `enabled` is informational) |
| `[mcp]` and `.mcp.json` | ✅ | MCP client servers (`apps/endside/src/cli/config.rs`) |
| `[memory_automation]` | ✅ | RAM-A long-term memory recall/ingest (`apps/endside/src/cli/config.rs`) |
| `[agent]` | ❌ | Agent roles (TUI/Daemon only) |
| `[lsp]` | ❌ | LSP configuration (CLI ignores it; TUI and Daemon both use it) |
| `[tui]`, `[tui.remote]` | ❌ | TUI-only keys and Remote TUI (TUI only) |
| `[channels]` | ❌ | Channel integration (Daemon only) |
| `[http]` | ❌ | HTTP API (Daemon only) |
| `[agents]` | ❌ | Multi-agent management (Daemon only; TUI reads a limited subset for default-agent validation only) |

---

## Complete CLI Configuration Example

CLI configuration is concise and clear. Here is a complete example:

```toml
# ~/.config/xiaoo/config.toml

[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

# Predefined subagent roles (CLI supported) ⭐
[subagent.code_reviewer]
description = "Code review specialist - reviews code quality"
prompt = "You are a code review specialist. Review for quality, security, and best practices."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
file_read = true
glob = true
grep = true

# Skills configuration (optional)
[skills]
dirs = ["~/.xiaoo/skills"]

# Context compression (optional)
[compact]
auto_compact_ratio = 0.75

# Tracing (optional)
[trace]
storage_backend = "stdout"

# Hooker (optional)
[hooker]
default = "all"
```

> **Note**: `[hooker] default` accepts only `all` or `none`
> (`crates/agent-types/src/hook/config/boot_configs.rs`); any other value
> is rejected by `config validate`.

> **Note**: The CLI's `[llm]` section only reads `provider`, `model`, `api_key_env`, `api_base`, `kvcache_enabled`, and `kvcache_debug_enabled`. Fields like `max_tokens` and `reasoning_effort` are not read from the config file in CLI mode — use `--reasoning-effort` as a CLI argument instead. The context window is resolved dynamically (no `context_window` config field). `llm.profiles`, `llm.active_profile`, and `llm.context_window` are **daemon-only** and are silently ignored by the CLI.

---

## CLI Usage

### Basic Usage

```bash
# Single execution
xiaoo --cli run -p "Count the characters in hello world"

# Use specific configuration file
xiaoo --cli run --config /path/to/config.toml -p "Your prompt"

# Show debug information
xiaoo --cli run --debug -p "Your prompt"

# Disable tool execution
xiaoo --cli run --no-tools -p "Just answer this question"

# Read a large UTF-8 prompt without passing its contents through argv
cat prompt.txt | xiaoo --cli run
xiaoo --cli run < prompt.txt
cat context.txt | xiaoo --cli run -p "Summarize this context"
```

When stdin is not a terminal (a pipe or redirected file), CLI reads UTF-8 text
from stdin to EOF and uses it as the prompt. If `-p/--prompt` is also provided,
stdin is appended after the argument text, separated by a newline when both are
nonempty. Stdin whitespace and newlines are preserved. Terminal stdin is not read;
an empty stdin leaves the argument prompt unchanged. Prompt reading is included
in the overall `--timeout` deadline. This avoids the operating system's argv size
limits without an additional flag.

### Parameter Description

| Parameter | Description | Default |
|------|------|--------|
| `-p, --prompt` | Prompt to send to agent. **Optional and repeatable** (`num_args = 1..`); repeats are joined into one prompt. Piped stdin is appended on a new line | — |
| `--config` | Configuration file path (also accepts `XIAOO_CONFIG` env var). `global` | `~/.config/xiaoo/config.toml` |
| `--debug` | Show intermediate process (turns, tool calls, etc.). `global` | false |
| `--provider` | Override provider in configuration file | - |
| `--model` | Override model in configuration file | - |
| `--api-key` | Override API key in configuration file / env | - |
| `--api-base` | Override API base URL in configuration file | - |
| `--system` | Override default system prompt | built-in `cli_default_system_prompt.txt` |
| `--max-turns` | Maximum number of turns | 10 |
| `--no-tools` | Disable tool execution | false |
| `--tools` | Comma-separated allowlist of tool names; empty/unset = all tools | - |
| `--reasoning-effort` | Reasoning effort: off, high, max | off |
| `--format` | Output format: `default` (human-readable) or `json` (one event object per line) | `default` |
| `--title` | Human-readable session title | - |
| `-s, --session` | Resume an existing session by ID | new random UUID |
| `--agent` | Agent ID to use for this run | `defaultagent` |
| `--attach` | Attach to a running daemon at the given URL instead of running locally | - |
| `--mcp-config` | Path to standard MCP JSON config. `global` | `.mcp.json` discovery |
| `-v, --version` | Show version number. `global` | false |

> `--config`, `--debug`, `--mcp-config`, and `-v/--version` are `global = true`
> clap args, so they can appear before or after the `run` subcommand
> (`apps/endside/src/cli/entry.rs`). The remaining flags are
> `run`-subcommand options (`entry.rs`). `XIAOO_CONFIG` env var falls
> back to the default `~/.config/xiaoo/config.toml` when neither `--config` nor
> the env var is set.

> `--provider`, `--model`, `--api-key`, and `--api-base` override only the
> top-level `[llm]` keys; they are not profile-aware.

> Behind a gateway with an untrusted certificate, every LLM request — chat, model
> list, embeddings — fails the TLS handshake until the CLI is run with
> `NODE_TLS_REJECT_UNAUTHORIZED=0`. Scope and caveats:
> [Self-signed gateways](./config_file_guide.md#self-signed-gateways-node_tls_reject_unauthorized).

### Subcommands

| Subcommand | Description |
|------|------|
| `run` | Run a single prompt through the AgentLoop |
| `serve` | Start a local daemon server (`--port`, `--hostname`). Forwards to an externally installed `xiaoo-daemon` binary (fails if it is not installed); defaults to port `4096`, not the daemon's own default `18080` (`entry.rs`). |
| `export <session_id>` | Export a session transcript from a running daemon (`--port`, `--client-id`). Known inconsistency: the CLI sends `GET /api/v1/runtimes/export/{session_id}` (`entry.rs`) while the daemon only registers `POST /api/v1/runtimes/export` with the runtime id in the JSON body (`apps/serverside/src/httpserver/router.rs`), so this subcommand currently fails against the daemon. |
| `debug` | Inspect resolved configuration and internal state |
| `skill` | Manage skills |

Source: the `Command` enum in `apps/endside/src/cli/entry.rs`.

---

## CLI and Subagent

CLI supports `[subagent]` configuration, allowing the main agent to delegate tasks to specialized subagents.

> **Note**: Tools configuration supports two formats. See [Configuration File Guide](./config_file_guide.md#subagent---predefined-subagent-roles-new) for details.

### Configuration Example

```toml
[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
file_read = true
glob = true
grep = true
```

> See the known-gap note in [config_file_guide.md](./config_file_guide.md#subagent---predefined-subagent-roles-new):
> `[subagent.<role>.tools]` is currently **not enforced** for subagent roles — it
> parses but is never applied by the session supervisor.

### Use Cases

```bash
# Code review task (main agent will automatically delegate to code_reviewer subagent)
xiaoo --cli run -p "Review my authentication module for security issues"

# Test writing task (main agent will automatically delegate to test_writer subagent)
xiaoo --cli run -p "Write comprehensive tests for user registration API"
```

### How It Works

When CLI has subagent configured:
1. Main agent receives subagent delegation rules in system prompt
2. Main agent detects if user request matches subagent description
3. If matched, main agent calls `spawn_subagent(subagent_role_id="xxx")`
4. Subagent executes asynchronously, main agent calls `join_subagent` to wait for results
5. Main agent returns results to user

---

## CLI vs Other Modes

| Feature | CLI | TUI | Daemon |
|------|-----|-----|--------|
| Running mode | Single command | Interactive UI | HTTP API service |
| Multi-role switching | ❌ | ✅ (Tab key) | ✅ |
| LSP diagnostics | ❌ | ✅ | ✅ (`[lsp]` is parsed and wired by the daemon) |
| Remote connection | ✅ (`--attach <url>`) | ✅ | ❌ |
| Channel integration | ❌ | ❌ | ✅ |
| Subagent delegation | ✅ | ✅ | ✅ |
| Session persistence | ✅ (`-s/--session <id>`, `export`) | ✅ | ✅ |
| Interactive Q&A | ❌ | ✅ | ✅ |

Notes:
- **LSP**: the daemon parses `[lsp]`
  (`apps/serverside/src/daemon_config.rs`) and builds an LSP registry when
  `enabled = true` (`daemon_config.rs`), wired into the runtime at
  `apps/serverside/src/daemon_runtime.rs`. The CLI does not read `[lsp]`.
- **Remote connection**: `xiaoo --cli run --attach <url>` talks to a running
  daemon instead of running locally (flag parsing in
  `apps/endside/src/cli/entry.rs`; implementation in `apps/endside/src/cli/attach.rs`).
- **Session persistence**: `-s/--session <id>` resumes/continues a session
  (`entry.rs`), and
  `xiaoo --cli export <session_id>` is intended to export a transcript from a
  running daemon, but the CLI's `GET /api/v1/runtimes/export/{session_id}`
  request does not match the daemon's `POST /api/v1/runtimes/export` route
  (`entry.rs` vs `router.rs`), so the export currently fails.

---

## FAQ

### Q: Does CLI support agent role configuration?

**A**: ❌ No. `[agent]` configuration only takes effect in TUI and Daemon. CLI does not support multi-role switching.

For multi-role functionality, use:
- **TUI**: `xiaoo` + Tab key switching
- **Daemon**: HTTP API + agent role configuration

### Q: Will CLI-configured subagents take effect?

**A**: ✅ Yes. CLI fully supports `[subagent]` configuration, and the main agent will automatically delegate tasks.

### Q: How to check if CLI configuration is loaded correctly?

**A**: Use the `--debug` parameter:
```bash
xiaoo --cli run --debug -p "test"
```
Output will show:
- Configuration file path
- Provider and model information
- Configuration values like max_turns

### Q: What scenarios is CLI suitable for?

**A**: CLI is suitable for:
- Single task execution
- Script integration
- Quick testing
- Automated workflows

Not suitable for:
- Long conversations
- Multi-role collaboration
- LSP diagnostics required
- Channel integration (Feishu/Telegram)

### Q: How does CLI handle long conversations?

**A**: By default each CLI run creates a new session (a fresh random UUID). Sessions **are** supported:

- **Resume**: pass `-s/--session <id>` to reuse an existing session ID
  (`apps/endside/src/cli/entry.rs`, consumed at `entry.rs`).
- **Export**: `xiaoo --cli export <session_id>` is intended to export a
  transcript from a running daemon, but its `GET /api/v1/runtimes/export/{session_id}`
  request does not match the daemon's `POST /api/v1/runtimes/export` route
  (`entry.rs` vs `router.rs`), so the export currently fails.
- **Remote**: `--attach <url>` runs the turn against a daemon that owns the
  session state (`entry.rs`).

For fully interactive long conversations, prefer:
- **TUI**: Supports session save and restore
- **Daemon**: Supports session persistence

---

## CLI Best Practices

### 1. Environment Variable Management

```bash
# Set API keys in ~/.bashrc or ~/.zshrc
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENROUTER_API_KEY="sk-or-..."
```

### 2. Concise Configuration File

```toml
# Only configure essentials
[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"
api_key_env = "ANTHROPIC_API_KEY"

# Configure subagents to improve task quality
[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
file_read = true
glob = true
grep = true
```

### 3. Usage Examples

```bash
# Quick code review
xiaoo --cli run -p "Review src/auth.rs for security issues"

# Quick test generation
xiaoo --cli run -p "Generate unit tests for user.rs"

# Simple Q&A (disable tools)
xiaoo --cli run --no-tools -p "Explain the difference between TCP and UDP"

# Debug mode to view execution process
xiaoo --cli run --debug -p "List all Python files in the project"
```

---

## Reference Links

- **General Configuration**: [config_file_guide.md](./config_file_guide.md)
- **Subagent Configuration**: [config_file_guide.md#subagent](./config_file_guide.md#subagent-预定义subagent角色)
- **TUI Configuration**: [tui_config.md](./tui_config.md)
- **Daemon Configuration**: [daemon_config.md](./daemon_config.md)
- **Quick Start**: [README.md](../README.md)

## LLM request timeout and retries

Streaming and non-streaming LLM calls share one retry policy: an initial attempt
plus at most **5 retries**, each limited to **20 seconds** for the entire request
(connection, headers and full response body). Continuous streaming does not
extend the per-attempt limit. A logical call has at most 6 attempts and a
120-second request budget, excluding hooks and tools. There is no additional
backoff or Retry-After sleep, and Provider/Agent layers do not stack retries.
Transient transport, stream, rate-limit and recoverable server errors are retried;
authentication/configuration errors and cancellation are not.

Exhaustion ends the current turn, even if an error hook offers recovery. TUI
shows the error and waits for new user input. CLI closes the session and exits
nonzero. Stream retries discard partial text/reasoning from failed attempts.

## CLI task deadline

`xiaoo --cli run` defaults to an overall deadline of **600 seconds**. Use
`--timeout <positive integer>` to change it for local or attach mode:

```bash
xiaoo --cli run --timeout 300 -p "Review the repository"
xiaoo --cli run --attach http://127.0.0.1:4096 --timeout 900 -p "Run the checks"
```

The same deadline covers asynchronous startup, all LLM requests and retries,
tool execution, and all agent turns. It does not reset when a new phase starts
or a stream produces output. It is independent of the LLM request timeout.

On expiry, local CLI cancels the turn, terminates CLI-owned process groups
(including running shell tools), and closes the session. Attach mode sends cancel
and close RPCs; a failed cancel does not skip close. The CLI exits nonzero with
a deadline error (an `error` event in JSON mode).

Session shutdown has a separate 30-second cleanup limit; attach cancellation
allows up to 5 seconds, and local MCP shutdown allows up to 5 seconds. Cleanup
may therefore extend process lifetime beyond the task deadline. Shutdown errors
are reported explicitly, including when an unreachable daemon prevents confirming
that its session closed. CLI-owned process groups are reaped before error exit.
