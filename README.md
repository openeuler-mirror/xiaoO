<div align="center">
  <img src="./img/logo.jpeg" width="180" alt="xiaoO" style="border-radius: 6px;">
</div>

# xiaoO - Open-source Intelligence Hub of AgentOS
## What is xiaoO?
It is the intelligence hub of AgentOS, delivering self-governing system management, seamless agent orchestration, and ready-to-use smart capabilities across all user channels. xiaoO turns the entire OS into the agent's home — every resource, every service, every capability, curated and served under one roof.

> The butler that is the house.

[![License](https://img.shields.io/badge/license-MulanPSL--2.0-blue.svg)](./License)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
<a href="https://gitcode.com/openeuler/xiaoO"><img src="https://img.shields.io/badge/version-v0.0.1-red" alt="Version v0.0.1" /></a>
---


## Prerequisites
- Cargo >= 1.7 installed

## Installation (From Source)

```bash
git clone https://gitcode.com/openeuler/xiaoO.git
cd xiaoO
cargo build --release
cargo install --path apps/xiaoo-app
```

Install to `~/.cargo/bin/xiaoo`, and ensure that `~/.cargo/bin` is in `PATH`.

## Quick Start
Create the configuration file `~/.config/xiaoo/config.toml`

```toml
[llm]
provider = "openrouter" # openai, anthropic, ollama, openrouter, deepseek, zai, ...
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY" # Read the API key from this environment variable
max_tokens = 128000  # MaxToken for LLM API
context_window = 128000 # Optional, used for session compression budget

[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer. Focus on security, performance, and maintainability."

[agent.code-reviewer.tools]
write = false
edit = false
```

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

In TUI, press `Tab` / `Shift+Tab` to switch agent role presets. When the current line starts with a slash command, `Tab` still performs slash completion.

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
# default port（0.0.0.0:8080）
xiaoo-app daemon

# Specify configuration file, address and port
xiaoo-app daemon --config /path/to/config.toml --host 127.0.0.1 --port 18080
```

More config details in [daemon.md](./docs/daemon_config.md)
