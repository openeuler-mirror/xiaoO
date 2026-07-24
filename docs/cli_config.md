# CLI Configuration Guide

> **Note**: This document focuses on CLI (`xiaoo --cli run`) configuration options and usage.
>
> For **common configuration items** (llm, subagent, skills, compact, trace, hooker, etc.), please refer to [Configuration File Guide](./config_file_guide.md).

---

## CLI Configuration Overview

CLI is the simplest running mode, supporting all common configuration items, but **does not support** the following specialized configurations:

| Configuration | CLI Support | Description |
|--------|---------|------|
| `[llm]` | ✅ | LLM provider configuration |
| `[subagent]` | ✅ | Predefined subagent roles ⭐ |
| `[skills]` | ✅ | Skills configuration |
| `[compact]` | ✅ | Context compression configuration |
| `[trace]` | ✅ | Tracing configuration |
| `[hooker]` | ✅ | Hooker configuration |
| `[operation_backend]` | ✅ | Operation backend configuration |
| `[vault]` | ✅ | Encrypted secrets storage (see vault_secrets_design.md) |
| `[agent]` | ❌ | Agent roles (TUI/Daemon only) |
| `[lsp]` | ❌ | LSP configuration (TUI only) |
| `[tui.remote]` | ❌ | Remote TUI (TUI only) |
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
read = true
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
default = "agent_moss"
```

> **Note**: The CLI's `[llm]` section only reads `provider`, `model`, `api_key_env`, `api_base`, `kvcache_enabled`, and `kvcache_debug_enabled`. Fields like `max_tokens` and `reasoning_effort` are not read from the config file in CLI mode — use `--reasoning-effort` as a CLI argument instead. The context window is resolved dynamically (no `context_window` config field).

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
```

### Parameter Description

| Parameter | Description | Default |
|------|------|--------|
| `-p, --prompt` | Prompt to send to agent | Required |
| `--config` | Configuration file path (also accepts `XIAOO_CONFIG` env var) | `~/.config/xiaoo/config.toml` |
| `--debug` | Show intermediate process (turns, tool calls, etc.) | false |
| `--provider` | Override provider in configuration file | - |
| `--model` | Override model in configuration file | - |
| `--api-key` | Override API key in configuration file / env | - |
| `--api-base` | Override API base URL in configuration file | - |
| `--system` | Override default system prompt | built-in `cli_default_system_prompt.txt` |
| `--max-turns` | Maximum number of turns | 10 |
| `--no-tools` | Disable tool execution | false |
| `--tools` | Comma-separated allowlist of tool names; empty/unset = all tools | - |
| `--reasoning-effort` | Reasoning effort: off, high, max | off |

> `--config` and `--debug` are `global = true` clap args, so they can appear
> before or after the `run` subcommand. `XIAOO_CONFIG` env var falls back to
> the default `~/.config/xiaoo/config.toml` when neither `--config` nor the env
> var is set.

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
read = true
glob = true
grep = true
```

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
| LSP diagnostics | ❌ | ✅ | ❌ |
| Remote connection | ❌ | ✅ | ❌ |
| Channel integration | ❌ | ❌ | ✅ |
| Subagent delegation | ✅ | ✅ | ✅ |
| Session persistence | ❌ | ✅ | ✅ |
| Interactive Q&A | ❌ | ✅ | ✅ |

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

**A**: Each CLI run is an independent session, session persistence is not supported. For long conversations, use:
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
read = true
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