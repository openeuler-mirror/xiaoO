# TUI Configuration Guide

> **Note**: This document focuses on TUI (`xiaoo`) specific configuration items.
>
> For **common configuration items** (llm, subagent, skills, compact, trace, hooker, etc.), please refer to [Configuration File Guide](./config_file_guide.md).

---

## TUI Configuration Overview

TUI supports all common configurations and has the following specific configuration items:

| Configuration | Description |
|--------|------|
| `[tui.remote]` | Remote TUI configuration (connect to remote daemon) |
| `tui.agent_order` | Ordered list of agent-role tabs shown by `Tab` cycling |
| `tui.redact_secrets_display` | Redact assistant-echoed secrets in the transcript display (default `false`) |
| `[lsp]` | LSP server configuration (real-time diagnostics, **disabled by default**) |
| `[agent]` | Agent role configuration (Tab key multi-role switching) |

> **Note**: TUI does not read the `[compact]` section. Adaptive context
> compression is only wired into CLI and Daemon. If a TUI conversation grows
> past the provider context window, switch to CLI/Daemon or start a new
> session. See `config_file_guide.md` for details.
>
> **Note**: Keybindings are **hard-coded** and cannot be changed through the
> configuration file. There is no keymap table, no `[keymap]`/`[keys]` section,
> and no validation layer for keys — see [Keybindings](#keybindings).

---

## TUI-specific Configuration Details

### [tui] - TUI Options

```toml
[tui]
agent_order = ["core", "code-reviewer", "planner"]  # Optional, Tab-cycle order
redact_secrets_display = false                      # Optional, default false
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `agent_order` | string array | `[]` | Order in which agent roles appear when cycling roles with `Tab`. Entries are matched case-insensitively against `[agent.<name>]` keys; the literal entry `core` names the built-in core agent (the role-less default). Ordering is a hint only — roles not listed here are appended after the listed ones, and `core` is appended at the end when it is not mentioned. Unknown names are ignored. |
| `redact_secrets_display` | boolean | `false` | Redact secrets echoed by the assistant in the TUI display. Only affects the display sink — raw secrets remain in history/snapshots regardless. Set `true` for shoulder-surf protection. |

Sources: `apps/endside/src/support/config.rs:80-91` (`TuiConfig`), `:89-90`
(`redact_secrets_display` defaults to `false`), tab ordering and the `core`
sentinel in `apps/endside/src/state/app_state.rs:1324-1378`, display wiring in
`apps/endside/src/gateway_api/runtime_request.rs:252`.

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
# Manual connection (default): the TUI connects only when you run /remote <url>
[tui.remote]
url = "http://192.168.1.100:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = false

# Auto-connect on startup: swap the values below into the [tui.remote] table
# above (a TOML file cannot contain the same table twice):
#   url = "http://daemon.example.com:18080"
#   bearer_token_env = "XIAOO_REMOTE_TOKEN"
#   auto_connect = true
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

The complete slash-command set (canonical names, `Tab`-completable):
`/connect`, `/cron`, `/delete`, `/dir`, `/load`, `/mcp`, `/new`, `/save`,
`/remote`, `/sandbox`, `/sessions`, `/skills`
(`apps/endside/src/input/slash_complete.rs:14-63`).

| Command | Description |
|------|------|
| `/remote <url>` | Connect to remote daemon |
| `/remote` | Open the remote-session history dialog (no argument) |
| `/remote status` | Show connection status |
| `/remote off` | Disconnect and return to local mode |
| `/remote close` | Close the current remote session on the daemon |
| `/sessions [session_id]` | List sessions on the connected daemon in a dialog, or switch directly to `session_id` |
| `/connect` | Open the provider/model selection dialog |
| `/new` | Clear the current conversation context and create a new session |
| `/mcp` | List configured MCP servers |
| `/skills` | List available skills |
| `/sandbox` | Switch the local sandbox backend |
| `/cron` | Manage cron jobs (view, configure, delete, enable/disable) |
| `/dir <path>` | Switch the current workspace directory |
| `/save [name]` | Save a manual checkpoint (auto-names when omitted) |
| `/load [name]` | Open the manual/auto snapshot list, or load a named manual snapshot |
| `/delete` | Delete the selected conversation from the current session |

Sources: `/remote` argument handling `apps/endside/src/input/event_key.rs:945-1015`
(`status`/`off`/`close`/URL), `/sessions` `:1044-1105`, `/new` `:609`,
`/connect` `:815`, `/mcp` `:849`, `/skills` `:837`, `/sandbox` `:831`,
`/cron` `:861`, `/save` `:702`, `/load` `:754`, `/delete` `:585`, `/dir`
`:874`.

---

### [lsp] - LSP Server Configuration

LSP (Language Server Protocol) provides real-time code diagnostics, error messages, and other features.

> **Important**: LSP is **disabled by default**. Set `enabled = true` to turn it
> on; a bare `[lsp]` section (or omitting the section) leaves diagnostics off.
> `LspConfig.enabled` is a `#[serde(default)] bool`, i.e. `false`
> (`apps/endside/src/support/config.rs:22-24`; the daemon schema agrees,
> `apps/serverside/src/config_schema.rs` → `lsp.enabled` default `false`).

#### Configuration Structure

```toml
[lsp]
enabled = true                       # Enable LSP (default FALSE)
disabled_servers = []                # List of disabled LSP servers

[[lsp.extra_servers]]
id = "custom-server"                 # Server ID
extensions = ["ext"]                 # BARE file extensions, NOT globs
command = "/path/to/server"          # Server startup command
args = []                            # Startup arguments
root_markers = ["marker-file"]       # Project root directory markers
language_id = "custom-lang"          # Language ID
```

> **`extensions` must be bare extensions, not globs.** Each entry is compared
> for exact equality against `Path::extension()` — the part after the final dot,
> with no dot and no wildcard. Write `"lua"`, not `"*.lua"`; a glob entry such
> as `"*.lua"` or `"*.ext"` **never matches any file** and the server is silently
> never started (`crates/lsp/src/manager.rs:112-120`, where the match is
> `c.extensions.contains(&ext.as_str())`; built-ins use bare `&["rs"]` at
> `crates/lsp/src/servers.rs:221`).

#### Built-in LSP Servers

XiaoO TUI has the following LSP servers built-in (no configuration required once `[lsp] enabled = true`):

| Server | Language | Auto-trigger Condition |
|--------|------|--------------|
| rust-analyzer | Rust | `Cargo.toml`, `*.rs` |
| gopls | Go | `go.mod`, `*.go` |
| clangd | C/C++ | `compile_commands.json`, `*.c`, `*.cpp` |
| pyright | Python | `pyproject.toml`, `*.py` |
| typescript-language-server | TypeScript/JavaScript | `tsconfig.json`, `package.json` |

The companion columns above are `root_markers` and the server's extension list
(`crates/lsp/src/servers.rs:217-270`).

#### Configuration Example

```toml
[lsp]
enabled = true                       # Enable LSP diagnostics

# Disable specific server
disabled_servers = ["pyright"]       # Example: disable pyright

# Add custom LSP server
[[lsp.extra_servers]]
id = "lua-language-server"
extensions = ["lua"]                 # Bare extension — NOT "*.lua"
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

Replace `<name>` with a concrete role id (for example `code-reviewer`) — the
placeholder form below is a schema sketch, not runnable TOML.

```toml
[agent.code-reviewer]
description = "Role description"            # Required
prompt = "System Prompt"                    # Optional
max_turns = 5                               # Optional, per-role turn cap

[agent.code-reviewer.tools]
bash = true                          # Keep this tool visible
file_write = false                   # Remove this tool from the visible set
```

The reserved role id `plan` is built in and **cannot** be overridden in the
config file: `install_builtin_agent_roles` bails when `[agent.plan]` is present
(`apps/endside/src/support/config.rs:433-436`).

#### `[agent.<name>.tools]` semantics — a visibility MASK

The `tools` map is **not** an allow-list. It is a mask applied on top of the
full tool set:

| Entry state | Effect |
|-------------|--------|
| `tool_name = false` | **Removes** that tool from the visible set |
| `tool_name = true` | **Keeps**/re-adds that tool in the visible set |
| tool name absent from the map | That tool **stays visible** (nothing is removed implicitly) |
| whole `tools` map absent or empty | **All** tools remain visible |
| an entry naming a **tool that does not exist** | **Silently skipped** — no error, no warning |

So `[agent.x.tools] bash = false` means "everything except `bash`", not
"only bash". There is no `deny_unknown_fields`, so an unknown tool name is
silently ignored rather than rejected.

Sources: `apps/serverside/src/daemon_runtime.rs:971-999`
(`resolve_allowed_tool_names`: empty map → all names; unknown names `continue`;
`false` removes, `true` inserts), mirrored TUI-side in
`apps/endside/src/gateway_api/runtime_request.rs:430-455`
(`resolve_visible_tool_names`).

**Valid builtin tool names** (the ones that actually exist; `read`, `write` and
`edit` are **NOT** valid tool names and would be silently skipped):

`ask_user_question`, `bash`, `count_text_length`, `file_edit`, `file_read`,
`file_write`, `glob`, `grep`, `join_subagent`, `lsp`, `print_hello_world`,
`send_file`, `skill`, `spawn_subagent`, `todo_write`, `webfetch`, `web_search`.

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
- `Tab` key - Switch agent role (also completes a slash command when the line starts with `/`)
- `Ctrl+T` - Switch reasoning effort (off/high/max), `apps/endside/src/input/event_key.rs:403`
- Status bar shows current agent role name

---

## Keybindings

All TUI keybindings are **hard-coded**. There is no keymap configuration table,
no config-file section for keys, and no validation layer: the readline/emacs
editing chords live in `handle_input_key`
(`apps/endside/src/input/core.rs:631-690`) and the app-level chords in
`apps/endside/src/input/event_key.rs`. The source itself states "These bindings
are hard-coded — there is no keymap configuration"
(`apps/endside/src/input/core.rs:638`).

### Input-line editing (readline / emacs)

| Chord | Action |
|-------|--------|
| `Ctrl+A` / `Ctrl+E` | Move to line start / line end |
| `Ctrl+B` / `Ctrl+F` | Move one character left / right |
| `Alt+B` / `Alt+F` | Move one word left / right |
| `Ctrl+D` | Delete one character forward |
| `Ctrl+W` / `Alt+D` | Delete one word backward / forward |
| `Ctrl+U` / `Ctrl+K` | Kill to line start / line end |

> **Behavior change**: `Ctrl+A` is **line start**, *not* select-all. Keyboard
> selection uses `Shift+Left`/`Shift+Right`/`Shift+Home`/`Shift+End` instead
> (`apps/endside/src/input/core.rs:495-527`).

Source: `apps/endside/src/input/core.rs:631-690`.

### App-level chords

| Chord | Action |
|-------|--------|
| `Enter` / `Shift+Enter` | Submit the input |
| `Ctrl+J` / `Alt+Enter` / `Ctrl+Enter` | Insert a soft newline |
| `Ctrl+P` / `Ctrl+N` | Previous / next input-history entry |
| `Ctrl+Insert` / `Ctrl+Shift+Insert` | Copy the active selection (input or transcript) |
| `Ctrl+X` | Cut the selected input text |
| `Ctrl+C` | Quit (or copy, when a selection is active) |
| `Ctrl+T` | Cycle reasoning effort |
| `Tab` | Cycle agent role / complete slash command / accept file mention |
| `Up` / `Down` | Input history when the input is empty, else caret movement |
| `PageUp` / `PageDown` | Scroll the transcript a page |
| `Esc` | Clear an active transcript selection |

Sources: submit `apps/endside/src/input/event_key.rs:1808-1815`; soft newline
`:1822-1832`; history `:1826-1836`; copy `:1838-1846`; cut `:1848-1851`;
`Ctrl+C` / `Ctrl+Insert` copy-vs-quit split `:55-98`; `Ctrl+T` `:403`; `Tab`
`:378-399`; `Up`/`Down` `:480-520`; `PageUp`/`PageDown` `:525-530`; `Esc`
`:498-501`.

> `Shift+Enter` only reaches the app on terminals that report key modifiers
> (kitty keyboard / CSI u); mainstream unix terminals fold it onto the same
> `0x0D` byte as plain `Enter`. `Ctrl+Shift+C` is normally intercepted by the
> terminal emulator, which is why `Ctrl+Insert` is the reliable copy key
> (`apps/endside/src/input/event_key.rs:55-73`).

---

## Complete TUI Configuration Example

Here is a complete example containing both common configuration and TUI-specific configuration:

```toml
# Common configuration (applies to CLI/TUI/Daemon)
[llm]
provider = "openrouter"
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"
max_tokens = 128000
reasoning_effort = "off"

# Predefined subagent roles (common configuration)
# Note: Tools configuration supports two formats. See config_file_guide.md for details.
[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
file_read = true
glob = true
grep = true

# Context compression (common configuration; CLI/Daemon only — TUI ignores [compact]).
# In remote (daemon) mode the daemon resolves [compact] from its own config;
# the section is OPTIONAL and defaults to warning=0.6 / auto_compact=0.75 /
# blocking=0.9 when omitted. Compression is never silently disabled.
# [compact]
# auto_compact_ratio = 0.75

# Tracing (common configuration)
[trace]
storage_backend = "moirai-sqlite"
db_path = "~/.xiaoo/traces.db"

# Skills (common configuration)
[skills]
dirs = ["~/.xiaoo/skills"]

# Hooker (common configuration). `default` accepts only "all" (default) or "none".
[hooker]
default = "all"

# TUI-specific configuration

# TUI options
[tui]
agent_order = ["core", "code-reviewer", "test-generator"]
redact_secrets_display = false

# Remote TUI configuration (optional)
[tui.remote]
url = "http://192.168.1.100:18080"
bearer_token_env = "XIAOO_REMOTE_TOKEN"
auto_connect = false

# LSP configuration (disabled unless enabled = true)
[lsp]
enabled = true
disabled_servers = []

# Add custom LSP server (extensions are BARE, never globs)
[[lsp.extra_servers]]
id = "lua-language-server"
extensions = ["lua"]
command = "lua-language-server"
args = []
root_markers = [".luarc.json"]
language_id = "lua"

# Agent role configuration
[agent.code-reviewer]
description = "Reviews code for best practices"
prompt = "You are a code reviewer."
max_turns = 8

[agent.code-reviewer.tools]
file_write = false
file_edit = false

[agent.test-generator]
description = "Generates comprehensive test cases"
prompt = "You are a test generator."

[agent.test-generator.tools]
bash = true
file_read = true
file_write = true
```

---

## TUI Startup

```bash
# Local mode
xiaoo

# Use specific configuration file
xiaoo --config /path/to/config.toml

# Use a specific MCP config file
xiaoo --mcp-config /path/to/.mcp.json

# Show usage
xiaoo --help
```

The TUI argument parser accepts **only** `--config` / `-c`, `--mcp-config`,
`--help` / `-h`. **Any other argument is rejected** with
`unsupported argument "..."` and a non-zero exit — in particular
`xiaoo --debug` does **not** exist and fails
(`apps/endside/src/main.rs:143-167`; the fall-through `bail!` is at `:163-165`).

---

## FAQ

### Q: What configurations does TUI support?

**A**: TUI supports:
- ✅ Common configurations it actually parses: llm, subagent, skills, trace, hooker
- ❌ `[compact]` — **not read by the TUI**; it is CLI/Daemon only (see the note above)
- ✅ TUI-specific configurations (`tui`, `tui.remote`, `lsp`, `agent`)

The TUI `Config` struct has no `compact` field
(`apps/endside/src/support/config.rs:42-77`), so a `[compact]` section in a
TUI-parsed file has no effect in local TUI mode.

### Q: What's the difference between Agent and Subagent?

**A**:
- **Agent**: Multi-role switching (Tab key), TUI-specific, for different personalities
- **Subagent**: Task delegation (spawn_subagent), supported in all modes, for specialized division of labor

Detailed explanation: [custom_agent.md](./custom_agent.md)

### Q: How to configure LSP diagnostics?

**A**:
- **Disabled by default** — you must set `enabled = true` under `[lsp]` to turn it on
- Built-in servers are auto-detected once LSP is enabled
- Can add custom LSP servers (`extensions` must be bare extensions, not globs)
- Can disable specific servers

### Q: How to use Remote TUI?

**A**:
1. Configure daemon side bearer auth
2. Configure TUI side remote URL and token
3. In TUI, input `/remote <url>` or set `auto_connect = true`

Detailed instructions: [remote_tui.md](./remote_tui.md)

### Q: Can I remap the keyboard shortcuts?

**A**: No. The editing chords (`Ctrl+A/E/B/F`, `Alt+B/F`, `Ctrl+D/W/U/K`), the
submit/newline keys, history keys, and copy/cut chords are hard-coded with no
configuration surface (`apps/endside/src/input/core.rs:631-690`,
`apps/endside/src/input/event_key.rs:1804-1851`). Adding a keymap is a known gap,
not a configuration option.

---

## Known Gaps

- **No configurable keymap.** Keybindings cannot be changed via configuration;
  there is no keymap table and no validation layer
  (`apps/endside/src/input/core.rs:638`).
- **`lsp.extra_servers.extensions` silently ignores globs.** A wildcard entry
  never matches and produces no warning (`crates/lsp/src/manager.rs:112-120`).
- **Unknown tool names in `[agent.<name>.tools]` are silently skipped.** The
  config structs do not use `deny_unknown_fields`, so a typo such as
  `read = true` (instead of `file_read = true`) is accepted and ignored
  (`apps/serverside/src/daemon_runtime.rs:984-990`).

---

## Reference Links

- **Common Configuration**: [config_file_guide.md](./config_file_guide.md)
- **Agent Role Development**: [custom_agent.md](./custom_agent.md)
- **Remote TUI**: [remote_tui.md](./remote_tui.md)
- **Skills Usage**: [skill_usage.md](./skill_usage.md)
- **Plugins Configuration**: [plugins.md](./plugins.md)
- **Quick Start**: [README.md](../README.md)
