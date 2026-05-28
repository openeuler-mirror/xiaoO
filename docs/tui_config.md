# TUI Configuration Guide

> **Note**: This document focuses on TUI (`xiaoo-tui`) specific configuration items.
>
> For **common configuration items** (llm, subagent, skills, compact, trace, hooker, etc.), please refer to [Configuration File Guide](./config_file_guide.md).

---

## TUI Configuration Overview

TUI supports all common configurations and has the following specific configuration items:

| Configuration | Description |
|--------|------|
| `[tui.remote]` | Remote TUI configuration (connect to remote daemon) |
| `[lsp]` | LSP server configuration (real-time diagnostics) |
| `[agent]` | Agent role configuration (Tab key multi-role switching) |

---

## TUI-specific Configuration Details

### [tui.remote] - Remote TUI Configuration

Remote TUI allows TUI to connect to remote daemon, enabling cross-machine collaboration.

Detailed usage instructions: [remote_tui.md](./remote_tui.md)

#### Configuration Structure

```toml
[tui.remote]
url = "http://daemon-host:18080"     # Remote daemon URL
bearer_token_env = "XIAOO_REMOTE_TOKEN"  # Bearer token environment variable
auto_connect = false                 # Whether to auto-connect on startup
```

#### Configuration Example

```toml
# Manual connection (default)
[tui.remote]
url = "http://192.168.1.100:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = false

# Auto-connect on startup
[tui.remote]
url = "http://daemon.example.com:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = true
```

#### Usage Flow

1. **Daemon side configuration** (Machine A):
   ```toml
   [http]
   bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN"
   ```

2. **TUI side configuration** (Machine B):
   ```toml
   [tui.remote]
   url = "http://daemon-host:18080"
   bearer_token_env = "XIAOO_REMOTE_TOKEN"
   ```

3. **Set environment variables** (use the same token on both sides):
   ```bash
   export XIAOO_HTTP_BEARER_TOKEN="your-secret-token"
   export XIAOO_REMOTE_TOKEN="your-secret-token"
   ```

4. **Connect in TUI**:
   - Auto-connect: When `auto_connect = true`, TUI connects automatically on startup
   - Manual connect: Input `/remote http://daemon-host:18080`

#### Command Description

| Command | Description |
|------|------|
| `/remote <url>` | Connect to remote daemon |
| `/remote status` | Show connection status |
| `/remote off` | Disconnect and return to local mode |
| `/new` | Reopen session in remote mode |

---

### [lsp] - LSP Server Configuration

LSP (Language Server Protocol) provides real-time code diagnostics, error messages, and other features.

#### Configuration Structure

```toml
[lsp]
enabled = true                       # Enable LSP (default true)
disabled_servers = []                # List of disabled LSP servers

[[lsp.extra_servers]]
id = "custom-server"                 # Server ID
extensions = ["*.ext"]               # File extension matching
command = "/path/to/server"          # Server startup command
args = []                            # Startup arguments
root_markers = ["marker-file"]       # Project root directory markers
language_id = "custom-lang"          # Language ID
```

#### Built-in LSP Servers

XiaoO TUI has the following LSP servers built-in (no configuration required):

| Server | Language | Auto-trigger Condition |
|--------|------|--------------|
| rust-analyzer | Rust | `Cargo.toml`, `*.rs` |
| pyright | Python | `pyproject.toml`, `*.py` |
| typescript-language-server | TypeScript/JavaScript | `tsconfig.json`, `package.json` |
| gopls | Go | `go.mod`, `*.go` |
| clangd | C/C++ | `compile_commands.json`, `*.c`, `*.cpp` |

#### Configuration Example

```toml
[lsp]
enabled = true                       # Enable LSP diagnostics

# Disable specific server
disabled_servers = ["pyright"]       # Example: disable pyright

# Add custom LSP server
[[lsp.extra_servers]]
id = "lua-language-server"
extensions = ["*.lua"]
command = "lua-language-server"
args = []
root_markers = [".luarc.json"]
language_id = "lua"
```

#### LSP Diagnostics Display

TUI status bar shows LSP diagnostics in real-time:
- Error count (E)
- Warning count (W)
- Hover tooltips

---

### [agent] - Agent Role Configuration

Agent roles allow switching between different agent personalities using Tab key in TUI, suitable for multi-role scenarios.

**Important distinction**:
- `[agent]` - Agent roles (TUI multi-role switching, not supported in CLI)
- `[subagent]` - Subagent roles (task delegation, supported in all modes)

Detailed development guide: [custom_agent.md](./custom_agent.md)

#### Configuration Structure

```toml
[agent.<name>]
description = "Role description"            # Required
prompt = "System Prompt"             # Required

[agent.<name>.tools]
tool_name = true                     # Allow using this tool
tool_name = false                    # Disallow using this tool
```

#### Configuration Example

```toml
# Code review role
[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer. Focus on security, performance, and maintainability."

[agent.code-reviewer.tools]
file_write = false                   # Code review does not allow file modification
file_edit = false

# Bug fix role
[agent.bug-fixer]
description = "Fixes bugs and improves code quality"
prompt = "You are a bug fixer. Identify and fix issues."

# Planning role
[agent.planner]
description = "Creates detailed implementation plans"
prompt = "You are a planner. Break down tasks into steps."

[agent.planner.tools]
bash = false                         # Planning phase does not execute commands
```

#### Usage

In TUI:
- `Tab` key - Switch agent role
- `Shift+Tab` - Switch reasoning effort (off/high/max)
- Status bar shows current agent role name

---

## Complete TUI Configuration Example

Here is a complete example containing both common configuration and TUI-specific configuration:

```toml
# Common configuration (applies to CLI/TUI/Daemon)
[llm]
provider = "openrouter"
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"
context_window = 128000

# Predefined subagent roles (common configuration)
[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
read = true
glob = true
grep = true

# Context compression (common configuration)
[compact]
auto_compact_ratio = 0.75

# Tracing (common configuration)
[trace]
storage_backend = "moirai-sqlite"
db_path = "~/.xiaoo/traces.db"

# Skills (common configuration)
[skills]
dirs = ["~/.xiaoo/skills"]

# Hooker (common configuration)
[hooker]
default = "audit_agent"

# TUI-specific configuration

# Remote TUI configuration (optional)
[tui.remote]
url = "http://192.168.1.100:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = false

# LSP configuration
[lsp]
enabled = true
disabled_servers = []

# Add custom LSP server
[[lsp.extra_servers]]
id = "lua-language-server"
extensions = ["*.lua"]
command = "lua-language-server"
args = []
root_markers = [".luarc.json"]
language_id = "lua"

# Agent role configuration
[agent.code-reviewer]
description = "Reviews code for best practices"
prompt = "You are a code reviewer."

[agent.code-reviewer.tools]
file_write = false
file_edit = false

[agent.test-generator]
description = "Generates comprehensive test cases"
prompt = "You are a test generator."

[agent.test-generator.tools]
bash = true
read = true
write = true
```

---

## TUI Startup

```bash
# Local mode
xiaoo-tui

# Use specific configuration file
xiaoo-tui --config /path/to/config.toml

# Debug mode
xiaoo-tui --debug
```

---

## FAQ

### Q: What configurations does TUI support?

**A**: TUI supports:
- ✅ All common configurations (llm, subagent, skills, compact, trace, hooker)
- ✅ TUI-specific configurations (tui.remote, lsp, agent)

### Q: What's the difference between Agent and Subagent?

**A**:
- **Agent**: Multi-role switching (Tab key), TUI-specific, for different personalities
- **Subagent**: Task delegation (spawn_subagent), supported in all modes, for specialized division of labor

Detailed explanation: [custom_agent.md](./custom_agent.md)

### Q: How to configure LSP diagnostics?

**A**:
- Enabled by default, no configuration needed (auto-detects project type)
- Can add custom LSP servers
- Can disable specific servers

### Q: How to use Remote TUI?

**A**:
1. Configure daemon side bearer auth
2. Configure TUI side remote URL and token
3. In TUI, input `/remote <url>` or set `auto_connect = true`

Detailed instructions: [remote_tui.md](./remote_tui.md)

---

## Reference Links

- **Common Configuration**: [config_file_guide.md](./config_file_guide.md)
- **Agent Role Development**: [custom_agent.md](./custom_agent.md)
- **Remote TUI**: [remote_tui.md](./remote_tui.md)
- **Skills Usage**: [skill_usage.md](./skill_usage.md)
- **Plugins Configuration**: [plugins.md](./plugins.md)
- **Quick Start**: [README.md](../README.md)