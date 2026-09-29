# XiaoO Configuration File Guide

Configuration file location: `~/.config/xiaoo/config.toml`

This document focuses on shared configuration items plus local client runtime settings.

> ⚠️ **Unknown keys are silently ignored.** No TOML config struct uses serde's
> `deny_unknown_fields`, so a misspelled or stale key — and a valid key written
> in a section that does not support it — parses successfully and is then
> dropped without warning. Only the `.mcp.json` reader rejects unknown fields
> (`crates/mcp/src/json_config.rs:62,73`). `xiaoo-daemon config validate`
> reports `"valid": true` even for ignored keys, so a passing validate is **not**
> proof that a key is honored.

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
| `[skills]` | Skills configuration (dirs/allow_scripts/disabled only) | [View Details](#skills---skills-configuration) |
| `[compact]` | Context compression strategy (CLI/Daemon only; TUI ignores this section) | [View Details](#compact---context-compression-strategy) |
| `[trace]` | Tracing/Observability | [View Details](#trace---tracingobservability) |
| `[hooker]` | Hooker configuration | [View Details](#hooker---hooker-configuration) |
| `[operation_backend]` | CLI/TUI operation backend configuration | [View Details](#operation_backend---operation-backend-configuration) |
| `[vault]` | Encrypted secrets storage (all modes; read via `xiaoo_shared::llm_secrets`; `enabled` is informational) | [View Details](#vault---encrypted-secrets-storage) |
| `[mcp]` and `.mcp.json` | MCP client servers, including RAM-A Streamable HTTP | [View Details](#mcp---model-context-protocol-client-configuration) |
| `[tui]` | TUI-only keys: `agent_order`, `redact_secrets_display` | [View Details](#tui---tui-only-keys) |
| `[memory_automation]` | Opt-in RAM-A long-term memory recall and ingest | [View Details](#memory_automation---opt-in-long-term-memory) |

---

## [mcp] - Model Context Protocol Client Configuration

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

xiaoO can connect to stdio, legacy SSE, and Streamable HTTP MCP servers.
For RAM-A, use a standard `.mcp.json` file so the bearer token stays in an
environment variable:

```json
{
  "mcpServers": {
    "ram-a": {
      "transport": "streamable_http",
      "url": "http://127.0.0.1:18081/mcp",
      "bearer_token_env": "RAM_A_XIAOO_TOKEN",
      "agent_id": "xiaoo",
      "timeout_ms": 30000
    }
  }
}
```

MCP servers can also be declared directly in `config.toml` with the
`[[mcp.servers]]` array-of-tables form (`crates/mcp/src/config.rs:6-10`):

```toml
[[mcp.servers]]
name = "ram-a"
transport = "streamable_http"
url = "http://127.0.0.1:18081/mcp"
bearer_token_env = "RAM_A_XIAOO_TOKEN"
agent_id = "xiaoo"
timeout_ms = 30000
enabled = true

[mcp.servers.effect]
reads_filesystem = false
writes_filesystem = false
network_access = true
side_effects = false
```

The `effect` profile declares which effects the server's tools have
(`crates/mcp/src/config.rs:56-58,115-124`). It defaults to the **most
conservative** assumption — all four flags `true` — so batches containing those
tools are serialised. Relax it for known read-only servers to allow parallel
execution. A `.mcp.json` entry has no `effect` key.

MCP JSON lookup order is:

1. `--mcp-config <path>`
2. `XIAOO_MCP_CONFIG`
3. workspace `.mcp.json`
4. `~/.config/xiaoo/mcp.json`

The selected file must parse and validate. Unknown fields are rejected.
`bearer_token_env` stores only the environment variable name; never write the
token value into JSON. RAM-A's Streamable HTTP endpoint is `/mcp` and uses
protocol version `2025-11-25`. xiaoO owns transport headers such as `Origin`,
`Authorization`, `X-Agent-ID`, `mcp-session-id`, and `mcp-protocol-version`;
do not put them in `.mcp.json` `headers`.

## [memory_automation] - Opt-in Long-term Memory

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

Automatic long-term memory is off by default. Enable it only after an MCP
server named by `server` is configured and exposes RAM-A `memory_search` and
`memory_ingest` tools.

```toml
[memory_automation]
enabled = true
server = "ram-a"
recall_top_k = 5
recall_token_budget = 512
context_messages = 4
queue_path = "memory-automation-queue.jsonl"
queue_capacity = 256
max_retries = 5
retry_backoff_ms = 250
allowed_agent_roles = ["main"]
```

Before a turn, xiaoO appends bounded recalled memory as
`<untrusted_long_term_memory>` system context and does not rewrite the user's
original message. After a successful turn, xiaoO writes ingest work to a
durable JSONL queue and retries with backoff. RAM-A/MCP failures degrade memory
behavior but must not fail a normal completed reply.

The TUI `Mem OK` indicator means the RAM-A connection and recent operations
are healthy. It does not mean every role is enabled: a turn whose active role
is absent from `allowed_agent_roles` deliberately skips recall and ingest.

---

## [llm] - LLM Provider Configuration

**Applicable to**: Basic keys CLI ✅ | TUI ✅ | Daemon ✅ — see the profile note below.

| `[llm]` key | CLI | TUI | Daemon |
|-------------|-----|-----|--------|
| `provider`, `model`, `api_key_env`, `api_base`, `kvcache_enabled`, `kvcache_debug_enabled` | ✅ | ✅ | ✅ |
| `max_tokens` | ❌ | ✅ | ✅ |
| `reasoning_effort` | ❌ | ✅ | ✅ (per-request HTTP field) |
| `profiles`, `active_profile`, `context_window` | ❌ | ❌ | ✅ |

### Native Model Profiles (Daemon only)

> ⚠️ **Daemon-only**: `[llm.profiles.*]`, `llm.active_profile`, and
> `llm.context_window` are read **only by the daemon**
> (`apps/serverside/src/daemon_config.rs:76-119`). The CLI `[llm]` struct has
> exactly six fields (`apps/endside/src/cli/config.rs:44-51`) and the TUI
> `LlmConfig` has no profile or `context_window` field
> (`apps/endside/src/support/config.rs:104-121`). In CLI and TUI these keys are
> **silently ignored** — no error, no warning (see the unknown-keys warning
> above). A CLI/TUI user who sets `active_profile` gets the `provider`/`model`
> values instead, with no diagnostic.

Use native profiles when more than one model is required. `active_profile`
selects the startup/default profile; disabled profiles remain in the file but
cannot be selected by a runtime.

```toml
[llm]
active_profile = "qwen"

[llm.profiles.qwen]
enabled = true
provider = "openai-compatible"
model = "qwen3.7-plus"
api_base = "https://example.com/compatible-mode/v1"
api_key_env = "QWEN_API_KEY"
context_window = 128000
max_tokens = 16384
reasoning_effort = "off"
kvcache_enabled = false
kvcache_debug_enabled = false

[llm.profiles.deepseek]
enabled = true
provider = "deepseek"
model = "deepseek-chat"
api_key_env = "DEEPSEEK_API_KEY"
```

`reasoning_effort` 是 Profile 的默认推理强度。HTTP/daemon 客户端省略单轮请求的
`reasoning_effort` 时使用此值；显式传入 `off`、`high` 或 `max` 时只覆盖当前请求。

Profile IDs are the keys below `llm.profiles`. When profiles exist,
`active_profile` is required and must reference an enabled profile.
Daemon clients can select an enabled profile for a session by sending
`llm.profile_id` in the runtime open request. The resolved profile ID is stored
with the session, so subsequent turns keep using it until a client explicitly
selects another profile.

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
1. Dynamic model query, for **any** provider whose profile sets
   `supports_model_catalog` (`crates/llm-client/src/models/mod.rs:79-99`) —
   this covers `openai`, `anthropic`, `gemini`, `ollama`, `zai`/`zhipu`,
   `deepseek`, `openrouter`, and others; not just gemini/anthropic/ollama
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
| `zai` (aliases `zhipu`, `bigmodel`, `z-ai`, `z.ai`, `zai-cn`, `zai-china`, `zai-global`, `glm-cn`) | Zhipu | `https://open.bigmodel.cn/api/paas/v4` | `ZHIPU_API_KEY` |
| `zai-coding-plan` (aliases `zhipu-coding-plan`, `zhipuai-coding-plan`) | OpenAI-compatible | `https://api.z.ai/api/coding/paas/v4` | `ZHIPU_API_KEY` |
| `deepseek` | OpenAI-compatible | `https://api.deepseek.com/v1` | `DEEPSEEK_API_KEY` |
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
file_read = true

# Format 2: Inline format (compact)
# tools = { "bash" = true, "file_read" = true }
```

> Tool keys must be real builtin tool names — `bash`, `file_read`, `file_write`,
> `file_edit`, `glob`, `grep`, `lsp`, `skill`, `todo_write`, `web_search`,
> `webfetch`, `spawn_subagent`, `join_subagent`, `ask_user_question`,
> `count_text_length`, `print_hello_world`, `send_file`. `file_read` is the real
> name (`crates/tool/src/impl/builtin/file_read/spec.rs:31`); `read`/`write`/`edit`
> are **not** tool names and are silently skipped (see the known gap below).

> ⚠️ **Known gap — subagent role `tools` is not enforced.** For agent roles
> (`[agent.<name>.tools]`) the map acts as a visibility mask. For **subagent**
> roles it is parsed and carried into `SubagentRoleRecord` but never applied:
> the session supervisor reads only `prompt`, `max_turns`, and `description`
> from the role (`apps/shared/src/gateway/session_supervisor.rs:345-356`). Only
> the built-in exploration subagent is narrowed, via a hardcoded allowlist.
> Do not rely on `[subagent.<role>.tools]` to restrict a subagent's tools.

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
file_read = true
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
# file_read = true
# file_write = true

# Option 3: Inline format
# [subagent.doc_writer]
# description = "Documentation specialist"
# max_turns = 3
# tools = { "bash" = true, "file_read" = true, "file_write" = true }
```

> **Note**: If `[subagent.<role_id>.tools]` is not configured, the subagent uses the global default tool permissions. See the known-gap note above: the map is currently not enforced for subagent roles even when configured.

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
disabled = ["legacy-review"]                           # Installed Skill IDs not loaded at runtime (optional)
```

The file-configurable surface of `[skills]` is exactly these three keys —
`dirs`, `allow_scripts`, `disabled` (`apps/serverside/src/daemon_config.rs:261-268`;
TUI `apps/endside/src/support/config.rs:182-189`).

> ⚠️ **Known gap — security knobs are not file-configurable and are silently
> dropped.** `SkillsConfig` in the code also has `audit_enabled`,
> `prompt_injection_mode`, `prompt_budget_ratio`, and
> `max_listing_description_chars` (`crates/skill/src/types/config.rs:8-12`), but
> **no** file-config section maps to them: every construction site fills them
> from `SkillsConfig::default()` via `..SkillsConfig::default()`
> (`apps/serverside/src/daemon_config.rs:884-898`,
> `apps/endside/src/support/config.rs:350-362`,
> `apps/endside/src/cli/skills.rs:55-67`). Writing `audit_enabled = true` in
> `config.toml` therefore **passes `config validate` with `"valid": true` and is
> then silently discarded** — skill security auditing stays off, and
> `prompt_injection_mode` stays `Compact` with `prompt_budget_ratio = 0.01`. Do
> not treat these as working config knobs.

For detailed skills usage instructions, please refer to [skill_usage.md](./skill_usage.md).

---

## [compact] - Context Compression Strategy

**Applicable to**: CLI ✅ | TUI ❌ | Daemon ✅

> **Note**: TUI does not read `[compact]`; in remote (daemon) mode the daemon
> builds the compression pipeline from its own `[compact]` section. The
> `[compact]` section is **optional** in both CLI and Daemon configs: when it
> is missing, a real `ContextManager` is constructed using built-in default
> thresholds (0.6 / 0.75 / 0.9). Compression is never silently disabled —
> omit the section to accept the defaults.

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
- `moirai-sqlite` - Store to SQLite database (the default; recommended for production)
- `noop` - No storage
- `stdout` - Output to standard output

The default backend is `moirai-sqlite` (`crates/trace/src/framework/config.rs:22,35`);
`noop` is **not** the default.

---

## [hooker] - Hooker Configuration

**Applicable to**: CLI ✅ | TUI ✅ | Daemon ✅

```toml
[hooker]
default = "all"                      # Default hooker mode: `all` or `none` only
enabled = []                         # Explicit hooker IDs to enable (optional)
disabled = []                        # Hooker IDs to disable (optional)
plugins = []                         # Plugin paths to load (optional)
policies = { }                       # Per-hooker policy values (optional)
max_prompt_chain_depth = 128         # Cross-turn send_prompt chain depth cap (optional, default 128)
```

`default` accepts **only `all` or `none`** (`all` is the default)
(`crates/agent-types/src/hook/config/boot_configs.rs:24-30`; the config schema
enum is `["all","none"]`, `apps/serverside/src/config_schema.rs:94`). Values
such as `agent_moss` are **rejected** by `config validate` with
``unknown variant `agent_moss` ``. The remaining keys come from the same struct
(`boot_configs.rs:33-52`); `max_prompt_chain_depth` defaults to 128
(`DEFAULT_MAX_PROMPT_CHAIN_DEPTH`, `boot_configs.rs:18`) and semantically
permits a chain of N turns total.

For detailed hooker configuration and plugin instructions, please refer to [plugins.md](./plugins.md).

---

## [tui] - TUI-only Keys

**Applicable to**: CLI ❌ | TUI ✅ | Daemon ❌

```toml
[tui]
agent_order = ["main", "plan"]       # Order in which agents are cycled (optional)
redact_secrets_display = false       # Redact secrets echoed by the assistant in the TUI display (default false)
```

`agent_order` and `redact_secrets_display` are parsed only by the TUI
(`apps/endside/src/support/config.rs:80-91`; declared in the config schema at
`apps/serverside/src/config_schema.rs:189-191`). `redact_secrets_display`
defaults to `false`, meaning the local TUI renders the transcript as-is; set it
to `true` for shoulder-surf protection. It affects the display sink only — raw
secrets remain in history and snapshots regardless.

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

The `[vault]` section controls local encrypted storage of API keys and tokens in
`llm_secrets.json` (sibling of `config.toml`), and all three modes read the
`use_sdf` key from it. The real API is
`xiaoo_shared::llm_secrets`: `save_llm_secret`, `delete_llm_secret`,
`auto_save_from_env`, `get_llm_secret`, `inject_llm_secrets_into_env`,
`inspect_secret_store`, and `llm_secrets_path`
(`apps/shared/src/llm_secrets.rs:40,58,79,228,245,257,219`). There is **no**
`llm_secrets::init_on_demand_secret_provider` symbol.

```toml
[vault]
enabled = false  # Informational only — does NOT gate secret persistence (see note)
use_sdf = false  # false = WhiteBox + AES-256-GCM (TEST ONLY); true = SDF 国密 (Kunpeng servers only)
```

| Field | Default | Description |
|-------|---------|-------------|
| `enabled` | `false` | **Informational only.** Reported by the daemon's vault status; not consulted when writing or reading secrets |
| `use_sdf` | `false` | Functional. Selects WhiteBox (`false`, test only) or SDF 国密 (`true`, Kunpeng only) |

> ⚠️ **`enabled` is not a gate.** `llm_secrets.rs` never reads it — the only key
> it parses from `[vault]` is `use_sdf`
> (`apps/shared/src/llm_secrets.rs:22-38`, used on every path at :40,58,228,245,257).
> The sole reader of `enabled` is daemon status reporting
> (`apps/serverside/src/vault_management.rs:74`). Secrets are written regardless:
> the TUI persists an API key unconditionally when one is entered
> (`apps/endside/src/services/provider.rs:132`). Setting `enabled = false`
> therefore does **not** prevent `llm_secrets.json` from being created.

> ⚠️ WhiteBox master key is currently all-zeros (test only). For production use
> SDF (`use_sdf = true`) on a Kunpeng server, or avoid persisting secrets and
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
