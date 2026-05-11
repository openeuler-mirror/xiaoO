<div align="center">
  <img src="./img/logo.jpeg" width="180" alt="xiaoO" style="border-radius: 6px;">
</div>

# xiaoO - Open-source Intelligence Hub of AgentOS
## What is xiaoO?
It is the intelligence hub of AgentOS, delivering self-governing system management, seamless agent orchestration, and ready-to-use smart capabilities across all user channels. xiaoO turns the entire OS into the agent's home — every resource, every service, every capability, curated and served under one roof.

At the runtime core, xiaoO ships a layered memory system and an adaptive context compression engine, so agents can stay stable across long conversations, tool-heavy execution, and multi-agent collaboration instead of collapsing under raw history growth.

[![License](https://img.shields.io/badge/license-MulanPSL--2.0-blue.svg)](./License)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
<a href="https://gitcode.com/openeuler/xiaoO"><img src="https://img.shields.io/badge/version-v0.0.1-red" alt="Version v0.0.1" /></a>


## Key Features
- Auto mode — --model auto / /model auto chooses both the model and thinking level for each turn
- Thinking-mode streaming — see DeepSeek reasoning blocks as the model works
- Full tool suite — file ops, shell execution, git, web search/browse, apply-patch, sub-agents, MCP servers
- Context managemant — context tracking, manual or configured compaction, and prefix-cache telemetry
- Reasoning-effort tiers — cycle through off → high → max with Shift + Tab
- Session save/resume managemant — checkpoint and resume long-running sessions
<!-- - Workspace rollback — side-git pre/post-turn snapshots with /restore and revert_turn, without touching your repo's .git -->
<!-- - Durable task queue — background tasks can survive restarts -->
- HTTP/SSE runtime API -http for headless agent workflows
<!-- - MCP protocol — connect to Model Context Protocol servers for extended tooling; please see docs/MCP.md -->
- LSP diagnostics — inline error/warning surfacing after every edit via rust-analyzer, pyright, typescript-language-server, gopls, clangd
- User memory — optional persistent note file injected into the system prompt for cross-session preferences. See in [Memory & Context Compression](./docs/memory_context_system.md).
- Localized UI — A clean, elegant, and user-friendly interface
- Live cost tracking — per-turn and session-level token usage and cost estimates; cache hit/miss breakdown
- Skills system — composable, installable instruction packs from GitHub with no backend service required
- Full-stack traceability: Hook points have been added at locations such as agent creation, before/after LLM calls, and before/after tool calls, enabling full-stack observability and allowing custom plugins to be inserted.
- Scheduled/triggered task: Supports long-term, scheduled/triggered tasks.

## Prerequisites
- Cargo >= 1.7 installed

## Installation (From Source)

```bash
git clone https://gitcode.com/openeuler/xiaoO.git
cd xiaoO
cargo build --release
cargo install --path apps/xiaoo-app
```

> **Note**: If you want to install with the security plugin loaded by default, use `./build.sh --release` instead. The `build.sh` script is a wrapper that prompts you to install the audit_agent security plugin.

Install to `~/.cargo/bin/xiaoo`, and ensure that `~/.cargo/bin` is in `PATH`.

For plugin installation and usage, please refer to [plugins.md](./docs/plugins.md).

## Quick Start
Create the configuration file `~/.config/xiaoo/config.toml`

```toml
[llm]
provider = "openrouter" # openai, anthropic, ollama, openrouter, deepseek, zai, ...
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY" # Read the API key from this environment variable
max_tokens = 128000  # Optional: max output tokens per response, defaults to 128000
context_window = 128000 # Optional: explicit total context budget override
reasoning_effort = "off" # Optional: off/high/max, defaults to off

[trace]
storage_backend = "moirai-sqlite"    # noop/stdout/moirai-sqlite
db_path = "/root/.config/xiaoo/traces.db"    # 仅当storage_backend 为 moirai-sqlite 时生效；未配置时为 ~/.moirai

```

`[llm].context_window` is optional. It sets an explicit total context budget override for runtime token budgeting and context compression. We resolves the effective value in this order:

1. Explicit user config: `[llm].context_window`
2. Dynamic model lookup: currently supported for `gemini`, `anthropic`, and `ollama`
3. Local fallback defaults:
   OpenAI-compatible / Ollama / Zhipu families default to `128000`
   Anthropic defaults to `200000`
   Gemini defaults to `1000000`

`[llm].reasoning_effort` controls the provider-side thinking/reasoning level:

| Value | Meaning | TUI color |
| --- | --- | --- |
| `off` | Disable extra reasoning controls where supported | Gray |
| `high` | Use a stronger reasoning/thinking setting | Yellow |
| `max` | Use the strongest reasoning/thinking setting | Red |

The TUI status bar shows the current value as `Think off/high/max`. Press `Shift+Tab` to cycle `off -> high -> max -> off` for the next turn. In CLI mode, use `xiaoo run --reasoning-effort high -p "..."` to override the config for one run.

Provider mapping is best-effort: OpenAI-compatible providers receive `reasoning_effort` only for `high`/`max`, Anthropic receives `thinking.budget_tokens`, Gemini receives `thinkingConfig.thinkingBudget`, and unsupported providers ignore the setting. `off` omits provider-specific reasoning fields so default requests keep each provider's native behavior.

Set environment variables

```bash
export OPENROUTER_API_KEY="sk-or-..."
```

```bash
# TUI Command
xiaoo-tui

# CLI Command
xiaoo run -p "Your Command"
```

In TUI, press `Tab` to switch agent role presets. Press `Shift+Tab` to switch think level. When the current line starts with a slash command, `Tab` still performs slash completion.

HTTP requests can also select an agent role preset by passing `agent` in the JSON body:

```json
{
  "text": "Review this patch for security issues",
  "channel": "http",
  "sender_id": "demo-user",
  "conversation_id": "demo-conv",
  "agent": "code-reviewer"
}
```
Example

```
$ xiaoo run -p 'Count "hello world" char numbers'
"hello world" has 11 chars.
```
## Run as Daemon

The Gateway operates in **daemon mode**, providing a RESTful API for external systems (such as Lark Webhook) to access.

```bash
# default port（0.0.0.0:18080）
xiaoo-app daemon

# Specify configuration file, address and port
xiaoo-app daemon --config /path/to/config.toml --host 127.0.0.1 --port 18080
```

More config details in [daemon.md](./docs/daemon_config.md)

Feishu channel setup guide in [feishu_deploy.md](./docs/feishu_deploy.md)
