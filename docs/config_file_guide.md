# XiaoO Configuration File Guide

Configuration file location: `~/.config/xiaoo/config.toml`

This document focuses on shared configuration items plus local client runtime settings.

> **Mode-specific Configuration**:
> - CLI: [cli_config.md](./cli_config.md)
> - TUI: [tui_config.md](./tui_config.md)
> - Daemon: [daemon_config.md](./daemon_config.md)

---

## Common Configuration Items Overview

Configuration items covered in this guide:

| Configuration | Description | Details |
|--------|------|----------|
| `[llm]` | LLM provider configuration | [View Details](#llm---llm-provider-configuration) |
| `[subagent]` ⭐ | Predefined subagent roles (all modes) | [View Details](#subagent---predefined-subagent-roles-new) |
| `[skills]` | Skills configuration | [View Details](#skills---skills-configuration) |
| `[compact]` | Context compression strategy (CLI/Daemon only; TUI ignores this section) | [View Details](#compact---context-compression-strategy) |
| `[trace]` | Tracing/Observability | [View Details](#trace---tracingobservability) |
| `[hooker]` | Hooker configuration | [View Details](#hooker---hooker-configuration) |
| `[operation_backend]` | CLI/TUI operation backend configuration | [View Details](#operation_backend---operation-backend-configuration) |
| `[vault]` | Encrypted secrets storage (all modes; read via `xiaoo_shared::llm_secrets`) | [View Details](#vault---encrypted-secrets-storage) |

---

## [llm] - LLM Provider Configuration

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

### Basic Configuration

```toml
[llm]
provider = "openrouter"              # Required: openai, anthropic, ollama, openrouter, deepseek, zai, minimax, kimi, minimax-coding-plan, kimi-coding-plan
model = "z-ai/glm-5"                 # Required: model name
api_key_env = "OPENROUTER_API_KEY"   # Recommended: read API key from environment variable
```

### Complete Configuration Items

```toml
[llm]
provider = "openrouter"              # Provider type (required)
model = "z-ai/glm-5"                 # Model name (required)
api_key_env = "OPENROUTER_API_KEY"   # API key environment variable (recommended)
api_base = "https://..."             # Custom API base URL (optional)
max_tokens = 128000                  # Maximum tokens per response (TUI/Daemon only; CLI ignores this field)
reasoning_effort = "off"             # Reasoning effort: off, high, max (TUI only; CLI uses --reasoning-effort; Daemon uses HTTP API field)
kvcache_enabled = false              # KV cache enabled (optional)
kvcache_debug_enabled = false        # KV cache debug (optional)
```

### Configuration Priority

The effective context window is resolved dynamically:
1. Dynamic model query (supported for gemini, anthropic, ollama)
2. Local fallback defaults

### Provider Types

Supported providers (see `crates/llm-client/src/provider_registry.rs` for the
authoritative list, including aliases). Provider names are case-insensitive.

| Provider | Protocol family | Default base URL | Default API key env |
|----------|-----------------|------------------|---------------------|
| `openai` | OpenAI-compatible | `https://api.openai.com/v1` | `OPENAI_API_KEY` |
| `anthropic` (alias `claude`) | Anthropic | `https://api.anthropic.com/v1` | `ANTHROPIC_API_KEY` |
| `gemini` (alias `google`) | Gemini | `https://generativelanguage.googleapis.com` | `GEMINI_API_KEY` |
| `ollama` | Ollama | `http://localhost:11434` | — (not required) |
| `zai` (aliases `zhipu`, `glm`, `bigmodel`, `z-ai`, `z.ai`, `zai-cn`, `zai-china`, `zai-global`, `glm-cn`) | Zhipu | `https://open.bigmodel.cn/api/paas/v4` | `ZHIPU_API_KEY` |
| `zai-coding-plan` (aliases `zhipu-coding-plan`, `zhipuai-coding-plan`) | OpenAI-compatible | `https://api.z.ai/api/coding/paas/v4` | `ZHIPU_API_KEY` |
| `deepseek` | OpenAI-compatible | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` |
| `openrouter` | OpenAI-compatible | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |
| `openai-compatible` | OpenAI-compatible | — (must set `api_base`) | `OPENAI_COMPATIBLE_API_KEY` |
| `groq` | OpenAI-compatible | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` |
| `mistral` | OpenAI-compatible | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| `together` | OpenAI-compatible | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` |
| `xai` (alias `xai-grok`) | OpenAI-compatible | `https://api.x.ai/v1` | `XAI_API_KEY` |
| `minimax` (alias `minimax-openai`) | OpenAI-compatible | `https://api.minimaxi.com/v1` | `MINIMAX_API_KEY` |
| `minimax-anthropic` | Anthropic | `https://api.minimaxi.com/anthropic/v1` | `MINIMAX_API_KEY` |
| `minimax-coding-plan` (aliases `minimax-code-plan`, `minimax-token-plan`) | OpenAI-compatible | `https://api.minimax.io/v1` | `MINIMAX_API_KEY` |
| `kimi` (aliases `moonshot`, `moonshot-ai`) | OpenAI-compatible | `https://api.moonshot.cn/v1` | `MOONSHOT_API_KEY` |
| `kimi-coding-plan` (aliases `kimi-code-plan`, `kimi-code`, `kimi-for-coding`) | OpenAI-compatible | `https://api.kimi.com/coding/v1` | `KIMI_API_KEY` |
| `gitcode` | OpenAI-compatible | `https://api-ai.gitcode.com/v1` | `GITCODE_API_KEY` |
| `local` | OpenAI-compatible | `http://localhost:8080/v1` | — (not required) |
| `other` | OpenAI-compatible | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` |

> `local` is a convenience alias for OpenAI-compatible local model servers. If
> your local server expects `/v1/chat/completions`, set `api_base` to
> `http://localhost:<port>/v1` explicitly to skip URL fallback probing.

---

## [subagent] - Predefined Subagent Roles ⭐ NEW

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

Through predefined subagent roles, the main agent can delegate specific tasks to specialized child agents.

### Configuration Structure

```toml
[subagent.<role_id>]
description = "Role description (used to match user requests)"  # Required
prompt = "Predefined system prompt"              # Optional
max_turns = 5                                  # Optional: maximum turn count

# Tools configuration - two formats supported:

# Format 1: Section format (recommended for clarity)
[subagent.<role_id>.tools]
bash = true
read = true

# Format 2: Inline format (compact)
# tools = { "bash" = true, "read" = true }
```

### Configuration Example

```toml
# Code review specialist
[subagent.code_reviewer]
description = "Code review specialist - focuses on code quality and best practices"
prompt = """You are a code review specialist. Your task is to:
1. Review code for quality, readability, and maintainability
2. Identify potential bugs and security issues
3. Suggest improvements following best practices"""
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
read = true
glob = true
grep = true

# Documentation specialist
# Example showing both format options for tools configuration:
[subagent.doc_writer]
description = "Documentation specialist"
max_turns = 3

# Option 1: Section format (shown above - no tools = uses global default)

# Option 2: Section format with explicit tools
# [subagent.doc_writer.tools]
# bash = true
# read = true
# write = true

# Option 3: Inline format
# [subagent.doc_writer]
# description = "Documentation specialist"
# max_turns = 3
# tools = { "bash" = true, "read" = true, "write" = true }
```

> **Note**: If `[subagent.<role_id>.tools]` is not configured, the subagent uses the global default tool permissions.

### Working Mechanism

After configuration, the main agent receives in system prompt:

```
## Subagent Delegation Rules

When handling user requests, you MUST check if there is a suitable predefined subagent role available.
Available predefined subagent roles:
- "code_reviewer": Code review specialist - focuses on code quality and best practices
- "test_writer": Test writing specialist - creates comprehensive test cases
```

Main agent call example:
```json
{
  "tool": "spawn_subagent",
  "arguments": {
    "subagent_role_id": "code_reviewer",
    "description": "Review authentication module"
  }
}
```

### Important Notes

- `description` is required, used to match user request scenarios
- `prompt` is optional, if not set, dynamically generated prompt is used
- `max_turns` prevents subagent from infinite loops
- `tools` restricts permissions, following least privilege principle
- Subagent does not recursively delegate (avoid multi-level nesting)

---

## [skills] - Skills Configuration

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

```toml
[skills]
dirs = ["~/.xiaoo/skills", "/path/to/custom/skills"]  # Skills directory list (optional)
allow_scripts = true                                    # Allow script-type skills (optional)
```

For detailed skills usage instructions, please refer to [skill_usage.md](./skill_usage.md).

---

## [compact] - Context Compression Strategy

**Applicable to**: CLI ✅ | TUI ❌ | Daemon ✅

> **Note**: TUI does not read `[compact]` and always runs without a compression
> pipeline (`compression_pipeline: None` in `apps/endside/src/gateway_api/runtime_request.rs`).
> Long TUI conversations are not auto-compacted; rely on CLI or Daemon mode when
> you need adaptive compression.

Controls context management strategy for long conversations:

```toml
[compact]
warning_ratio = 0.6                  # History ratio entering warning stage
auto_compact_ratio = 0.75            # Ratio that triggers automatic compression
blocking_ratio = 0.9                 # Ratio entering blocking stage
summary_max_tokens = 1024            # Token budget for summary
summary_preserve_tail = 4            # Recent messages to preserve after summary
snip_stale_after_ms = 3600000        # Stale message snip timeout (milliseconds)
snip_preserve_tail = 6               # Messages to preserve during snip
collapse_preserve_tail = 4           # Messages to preserve during collapse
summary_llm_max_tokens = 4096        # Summary LLM call max_tokens
```

---

## [trace] - Tracing/Observability

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

```toml
[trace]
storage_backend = "moirai-sqlite"    # Storage backend: noop, stdout, moirai-sqlite
db_path = "~/.xiaoo/traces.db"       # SQLite database path (for moirai-sqlite)
```

**storage_backend types**:
- `noop` - No storage (default)
- `stdout` - Output to standard output
- `moirai-sqlite` - Store to SQLite database (recommended for production)

---

## [hooker] - Hooker Configuration

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

```toml
[hooker]
default = "audit_agent"              # Default hooker mode: None, audit_agent, etc.
```

For detailed hooker configuration and plugin instructions, please refer to [plugins.md](./plugins.md).

---

## [operation_backend] - Operation Backend Configuration

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ❌

Daemon mode reads operation backend configuration from
`[server.operation_backend]`; see [Daemon Configuration](./daemon_config.md).

```toml
[operation_backend]
kind = "local"                       # Operation backend kind for local clients
options = { ... }                    # Backend-specific options
```

On macOS, the local backend can enable Seatbelt isolation:

```toml
[operation_backend]
kind = "local"

[operation_backend.options.isolation]
kind = "macos_seatbelt"
allow_network = false
```

For details, see [macOS Seatbelt Isolation](../crates/operation_backend/docs/seatbelt/README.md).

On Linux, the local backend can enable Bubblewrap isolation:

```toml
[operation_backend]
kind = "local"

[operation_backend.options.isolation]
kind = "linux_bubblewrap"
allow_network = false
```

For details, see [Linux Bubblewrap Isolation](../crates/operation_backend/docs/bubblewrap/README.md).

In the TUI, Bubblewrap Bash failures that resolve to an existing host path (or a
write target under an existing host parent directory) can prompt for a session
grant automatically, then retry the failed command after approval.

---

## [vault] - Encrypted Secrets Storage

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

The `[vault]` section toggles local encrypted storage of API keys and tokens in
`llm_secrets.json` (sibling of `config.toml`). All three modes read this section
through `xiaoo_shared::llm_secrets::init_on_demand_secret_provider`, which parses
the raw `[vault]` table from the TOML file directly.

```toml
[vault]
enabled = false  # true = persist secrets to llm_secrets.json encrypted with WhiteBox (use_sdf=false) or SDF (use_sdf=true)
use_sdf = false   # false = WhiteBox + AES-256-GCM (TEST ONLY); true = SDF 国密 (Kunpeng servers only)
```

| Field | Default | Description |
|-------|---------|-------------|
| `enabled` | `false` | Persist API keys/tokens to `llm_secrets.json` and decrypt on demand |
| `use_sdf` | `false` | Choose WhiteBox (`false`, test only) or SDF 国密 (`true`, Kunpeng only) |

> ⚠️ WhiteBox master key is currently all-zeros (test only). For production use
> SDF (`use_sdf = true`) on a Kunpeng server, or leave `enabled = false` and
> supply credentials through environment variables. Full design, file layout,
> and API reference: [vault_secrets_design.md](./vault_secrets_design.md).

---

## Configuration Loading Mechanism

### File Path Priority

1. `--config <PATH>` command line argument
2. `XIAOO_CONFIG` environment variable
3. `~/.config/xiaoo/config.toml` default path

### API Key Security Best Practices

**Recommended approach**:
- ✅ Use `api_key_env` to reference environment variables
- ✅ Set environment variables in shell configuration files

**Not recommended**:
- ❌ Write API keys directly in configuration files
- ❌ Commit configuration files to version control systems

```bash
# Set in ~/.bashrc or ~/.zshrc
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENROUTER_API_KEY="sk-or-..."
export FEISHU_APP_SECRET="..."
export TELEGRAM_BOT_TOKEN="..."
```

---

## FAQ

### Q: Will CLI-configured subagents take effect?

**A**: ✅ Yes. `[subagent]` configuration applies to CLI, TUI, and Daemon modes.

### Q: Do I need to restart after configuration changes?

**A**:
- CLI: Reloads configuration on each run
- TUI: Need to restart TUI
- Daemon: Need to restart daemon process

### Q: How to check if current configuration is loaded correctly?

**A**:
- CLI: Use `--debug` parameter to view loading logs
- TUI: View provider/model information in status bar
- Daemon: Check configuration parsing information in startup logs

### Q: Can different modes use the same configuration file?

**A**: ✅ Yes. Configuration files are shared, and each mode reads configuration items it supports.

---

## Reference Links

### Common Configuration References
- This document: Common configuration items (llm, subagent, skills, etc.)
- [skill_usage.md](./skill_usage.md) - Detailed skills usage instructions
- [plugins.md](./plugins.md) - Hooker and plugin configuration

### Mode-specific Configuration
- **CLI**: [cli_config.md](./cli_config.md)
- **TUI**: [tui_config.md](./tui_config.md)
- **Daemon**: [daemon_config.md](./daemon_config.md)

### Quick Start
- [README.md](../README.md) - Quick Start and basic examples
