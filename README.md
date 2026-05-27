<div align="center">
  <img src="./img/logo.jpeg" width="180" alt="xiaoO" style="border-radius: 6px;">
</div>

# xiaoO

[English](./README.md) | [中文](./README.zh-CN.md)

Open-source intelligence hub for AgentOS.

[![License](https://img.shields.io/badge/license-MulanPSL--2.0-blue.svg)](./License)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-v0.1.0-red.svg)](https://gitcode.com/openeuler/xiaoO)

## What is xiaoO?

xiaoO is the intelligence hub of AgentOS. It provides a self-governing agent runtime for system management, agent orchestration, tool execution, memory, context compression, and multi-channel access.

At its core, xiaoO turns the operating system into a practical home for agents: files, shell commands, Git, web access, LSP diagnostics, skills, hooks, channels, and runtime telemetry are exposed through one coordinated agent loop.

The runtime also includes a layered memory and adaptive context-compression system, so long conversations, tool-heavy tasks, and multi-agent collaboration can remain stable without being overwhelmed by raw history growth.

## Key Features

- Agent runtime hub: CLI, TUI, daemon, HTTP API, and channel integrations.
- Full tool suite: file operations, shell execution, Git, web search/browse, patch application, sub-agents, and extensible tool manifests.
- Adaptive context management: token-budget tracking, configured compaction, forced recovery after context-limit errors, and prefix-cache telemetry.
- Streaming reasoning: provider reasoning/thinking deltas can be surfaced while the model works.
- Reasoning-effort tiers: switch among `off`, `high`, and `max`; the TUI cycles them with `Shift+Tab`.
- Session management: save and resume long-running work.
- LSP diagnostics: inline errors and warnings after edits through servers such as `rust-analyzer`, `pyright`, `typescript-language-server`, `gopls`, and `clangd`.
- Skills system: installable instruction packs loaded from local directories or Git sources.
- Hook and plugin system: lifecycle hook points around agent creation, LLM calls, and tool calls for audit, policy, traceability, and custom extensions.
- Observability: live token/cost tracking and trace storage through `noop`, `stdout`, or `moirai-sqlite`.
- Scheduled and triggered tasks: long-running automation workflows can be attached to the runtime.
- Localized UI: a clean terminal interface designed for daily agent work.

## Prerequisites

- Rust toolchain with Cargo installed.
- A supported LLM provider account or a local model endpoint.
- Provider credentials available through environment variables or the xiaoO configuration file.

## Installation From Source

```bash
git clone https://gitcode.com/openeuler/xiaoO.git
cd xiaoO
cargo build --release
cargo install --path apps/xiaoo-app
```

This installs the application binaries into `~/.cargo/bin`. Make sure `~/.cargo/bin` is in your `PATH`.

If you want the interactive security-plugin prompt during build, use:

```bash
./build.sh --release
```

The build wrapper can install the `audit_agent` hooker, which audits tool execution for risky operations. Plugin installation details are available in [docs/plugins.md](./docs/plugins.md).

## Quick Start

Create `~/.config/xiaoo/config.toml`:

```toml
[llm]
provider = "openrouter"              # openai, anthropic, ollama, openrouter, deepseek, zai, minimax, kimi, minimax-coding-plan, kimi-coding-plan, ...
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"   # Read the API key from this environment variable
max_tokens = 128000                  # Optional, max output tokens per response
context_window = 128000              # Optional, explicit total context budget override
reasoning_effort = "off"             # Optional: off, high, or max

[trace]
storage_backend = "moirai-sqlite"    # noop, stdout, or moirai-sqlite
db_path = "~/.xiaoo/traces.db"       # Used when storage_backend is moirai-sqlite
```

Set your provider credential:

```bash
export OPENROUTER_API_KEY="sk-or-..."
```

Setup custom api url for local LLM:

```toml
[llm]
provider = "deepseek-local"
model = "deepseek-v4-flash"
api_base = "http://localhost:8000/v1/"
api_key_env = "LLM_API_KEY"
```

Run xiaoO:

```bash
# Terminal UI
xiaoo-tui

# Single-shot CLI
xiaoo run -p "Count the characters in hello world"
```

Example CLI output:

```text
"hello world" has 11 characters.
```

## Context Window

`[llm].context_window` is optional. It sets an explicit total context budget used by token budgeting and context compression. xiaoO resolves the effective value in this order:

1. Explicit user config: `[llm].context_window`
2. Dynamic model lookup, currently supported for `gemini`, `anthropic`, and `ollama`
3. Local fallback defaults:
   - OpenAI-compatible, Ollama, and Zhipu families default to `128000`
   - Anthropic defaults to `200000`
   - Gemini defaults to `1000000`

More details are available in [Memory & Context Compression](./docs/memory_context_system.md).

## Reasoning Effort

`[llm].reasoning_effort` controls provider-side thinking or reasoning where supported.

| Value | Meaning | TUI color |
| --- | --- | --- |
| `off` | Disable extra reasoning controls where supported | Gray |
| `high` | Use a stronger reasoning/thinking setting | Yellow |
| `max` | Use the strongest reasoning/thinking setting | Red |

The TUI status bar shows the current value as `Think off/high/max`. Press `Shift+Tab` to cycle `off -> high -> max -> off` for the next turn. In CLI mode, use:

```bash
xiaoo run --reasoning-effort high -p "Explain this repository"
```

Provider mapping is best-effort. OpenAI-compatible providers receive `reasoning_effort` for `high` and `max`; Anthropic receives `thinking.budget_tokens`; Gemini receives `thinkingConfig.thinkingBudget`; unsupported providers ignore the setting. `off` omits provider-specific reasoning fields so default requests keep each provider's native behavior.

## Skills

xiaoO loads skills from `~/.xiaoo/skills` by default. Each skill is a reusable instruction pack backed by `SKILL.md` or `SKILL.toml`.

```bash
xiaoo skill list
xiaoo skill show <name>
xiaoo skill audit <path>
xiaoo skill install ./my-skill/
xiaoo skill install https://github.com/user/my-skill.git
xiaoo skill remove <name>
```

See [docs/skill_usage.md](./docs/skill_usage.md) for the full skill workflow.

## Run as a Daemon

xiaoO can run as a daemon and expose a REST API for external systems such as Feishu, Telegram, or custom services.

```bash
# Default address: 0.0.0.0:18080
xiaoo-app daemon

# Specify configuration file, host, and port
xiaoo-app daemon --config /path/to/config.toml --host 127.0.0.1 --port 18080
```

HTTP requests can select an agent role preset by passing `agent` in the JSON body:

```json
{
  "text": "Review this patch for security issues",
  "channel": "http",
  "sender_id": "demo-user",
  "conversation_id": "demo-conv",
  "agent": "code-reviewer"
}
```

More daemon configuration details are in [docs/daemon_config.md](./docs/daemon_config.md).

## More Documentation

- [Memory & Context Compression](./docs/memory_context_system.md)
- [Plugin System](./docs/plugins.md)
- [Skill Usage](./docs/skill_usage.md)
- [Custom Agents](./docs/custom_agent.md)
- [Remote TUI](./docs/remote_tui.md)
- [Feishu Deployment](./docs/feishu_deploy.md)
- [Telegram Deployment](./docs/telegram_deploy.md)

## License

xiaoO is licensed under [MulanPSL-2.0](./License).
