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
| `agent_order` | string array | `[]` | Order in which agent roles appear when cycling roles with `Tab`. Entries are matched case-insensitively against `[agent.<name>]` keys; the literal entry `core` names the built-in core agent (the role-less default). When `core` is not mentioned in the list it is placed **first**, ahead of all listed roles; roles not listed here are appended after the listed ones (`app_state.rs`). Unknown names are ignored. |
| `redact_secrets_display` | boolean | `false` | Redact secrets echoed by the assistant in the TUI display. Only affects the display sink — raw secrets remain in history/snapshots regardless. Set `true` for shoulder-surf protection. |

Sources: `apps/endside/src/support/config.rs` (`TuiConfig`), tab ordering and
the `core` sentinel in `apps/endside/src/state/app_state.rs`, display
wiring in `apps/endside/src/gateway_api/runtime_request.rs`.

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
(`apps/endside/src/input/slash_complete.rs`).

| Command | Description |
|------|------|
| `/remote <url>` | Connect to remote daemon |
| `/remote` | Open the remote-session history dialog (no argument) |
| `/remote status` | Show connection status |
| `/remote off` | Disconnect and return to local mode |
| `/remote close` | Close the current remote session on the daemon |
| `/sessions [session_id]` | List sessions on the connected daemon in a dialog, or switch directly to `session_id` |
| `/connect` | Open the provider/model selection dialog; the model list is fetched live from the provider's official model-list endpoint |
| `/new` | Clear the current conversation context and create a new session |
| `/mcp` | List configured MCP servers |
| `/skills` | List available skills |
| `/sandbox` | Switch the local sandbox backend |
| `/cron` | Manage cron jobs (view, configure, delete, enable/disable) |
| `/dir <path>` | Switch the current workspace directory (alias: `/cd`); local mode only — in remote mode the command is refused because the session's workspace is bound on the daemon at open time |
| `/save [name]` | Save a manual checkpoint (auto-names when omitted) |
| `/load [name]` | Open the manual/auto snapshot list, or load a named manual snapshot |
| `/delete` | Delete the selected conversation from the current session |

Sources: all slash-command dispatch (`/remote` including its `status`/`off`/
`close`/URL argument handling, `/sessions`, `/new`, `/connect`, `/mcp`,
`/skills`, `/sandbox`, `/cron`, `/save`, `/load`, `/delete`) lives in
`apps/endside/src/input/event_key.rs`; `/dir` (and its `/cd` alias) is resolved in
`apps/endside/src/services/workspace.rs`.

#### Provider / model list refresh (`/connect`)

The dialog shows two panes (providers | models) and fetches the provider's
official model list live: when it opens, and whenever the highlighted provider
changes, the TUI asks that provider for its own official model list
(`GET {api_base}/models` for OpenAI-compatible providers, and the provider's
native shape for Anthropic / Gemini / Ollama) and replaces the provider's list
with the result.

- `Ctrl+R` — force a refresh of the highlighted provider's list, ignoring both
  the cache TTL and the failure back-off (e.g. right after entering an API key).
- Paging: `PgUp` / `PgDn` move the focused pane by one screen and `Home` / `End`
  jump to the first / last item. On a normal page the highlighted row lands at the
  top of the new screen, and paging **never moves it backwards** — see the last
  bullet below. The page size is the row count the renderer measured for the list
  (`ProviderDialog::page_height`, i.e. one visible screen) rather than a fixed
  constant, and the pane's visible window is written back after every frame, so a
  page always starts where the screen does. When no list row is visible — before the
  first render of a pane, or on a terminal so short that the dialog is squeezed to
  zero content rows — the step degrades to a single row instead of getting stuck. `Up` / `Down` keep single-row scrolling (they never
  jump a page). Two consequences worth knowing:
  - The last screen is aligned to the end of the list instead of showing a
    half-empty page, so the **final `PgDn` jump is shorter than a screen** and can
    overlap the previous one. If the highlight is already inside that last screen,
    only the window moves and the highlight stays where it is (a `PgDn` that moves
    the cursor *up* would be more confusing than a short jump). Once the window is
    already pinned at the end — or when the whole list fits in a single screen —
    `PgDn` does nothing at all, not even a model-list refresh.
  - A pane's window can sit wherever the renderer left it (after a list
    replacement or a deep preselection), so `PgUp` from such a position lands on
    the previous *grid* page — a multiple of the page size, at most one page above
    the screen you just saw. Aligning to a grid is what keeps repeated
    `PgUp`/`PgDn` from drifting.

  `PgUp` / `PgDn` in the providers pane can change the highlighted provider and
  therefore follows the same refresh policy as `Up` / `Down`; paging inside the
  models pane never issues a request.
- Where the list on screen comes from, as shown by the dialog itself:
  - The models pane title always carries the row count — `Models (12)` — and gains
    a `· refreshing…` suffix while a refresh is in flight and the pane is not empty.
  - An empty pane shows `Loading models...` while a fetch is running and
    `(no models available)` otherwise.
  - The muted status line under the panes explains the list in use:
    `Fetching the model list from the provider…` while loading,
    `Official model list · N models` once the official list has been applied,
    `<reason> (showing the existing list)` for a failure that kept the current list,
    `Failed to fetch the official model list: <reason>` (rendered in the error
    colour) when a fetch failed and the pane has nothing else to show, and
    `This provider has no model catalog (showing the built-in list)` for providers
    whose profile declares no catalog. While the built-in list is simply in use,
    the status line stays empty.
- Refresh policy: 5-minute TTL after a successful fetch, a 30-second back-off
  after a failure, and no automatic retry for providers whose profile declares no
  model catalog (`minimax`, `minimax-anthropic`, `gitcode`) — `Ctrl+R` still runs
  that decision once, short-circuiting before any HTTP request. At most two fetches
  are in flight at any time (counted globally, so the cap also holds across
  `/new`); providers beyond the cap keep the list they already have and are fetched the
  next time they are highlighted.
- Before the first successful fetch the built-in list is shown, so the dialog is
  usable offline. Once an official list has been fetched it stays in effect for
  the rest of the session — a later failure shows a hint line but never restores
  or clears the list.
- The active model is pinned to the top (`<id> (configured)`) when the official
  list does not contain it. The selection follows the model id across a refresh, so
  the cursor only moves when the highlighted model disappeared from the new list;
  it then falls back to the first row (the pinned entry, when the official list did
  not contain the configured model). Moving the provider highlight away and back
  also resets the cursor to the first row.
- The list is not cached on disk: restarting the TUI re-fetches it.

Sources: fetch pipeline `apps/endside/src/services/provider.rs` (request/outcome
types + `fetch_provider_models`, which calls the `xiaoo-shared` facade
`apps/shared/src/gateway/llm_assembly.rs`); state
`apps/endside/src/state/chat.rs` (`ModelListState`, TTL/back-off constants);
orchestration `apps/endside/src/app/core.rs` (`start_model_fetch`,
`apply_model_fetch_outcome`); triggers `apps/endside/src/input/event_key.rs`
(`open_provider_selection_dialog`, and the `Ctrl+R` / paging arms in
`handle_provider_selection_key`); paging state
`apps/endside/src/render/provider_dialog.rs` (`page_height`, `page_up` /
`page_down` / `select_first` / `select_last`); rendering
`apps/endside/src/render/overlay.rs` (`render_provider_dialog` writes
`page_height` and picks the key-hint row variant for the available width).

---

### [lsp] - LSP Server Configuration

LSP (Language Server Protocol) provides real-time code diagnostics, error messages, and other features.

> **Important**: LSP is **disabled by default**. Set `enabled = true` to turn it
> on; a bare `[lsp]` section (or omitting the section) leaves diagnostics off.
> `LspConfig.enabled` is a `#[serde(default)] bool`, i.e. `false`
> (`apps/endside/src/support/config.rs`; the daemon schema agrees,
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
> never started (`crates/lsp/src/manager.rs`, where the match is
> `c.extensions.contains(&ext.as_str())`; built-ins use bare `&["rs"]` at
> `crates/lsp/src/servers.rs`).

#### Built-in LSP Servers

XiaoO TUI has the following LSP servers built-in (no configuration required once `[lsp] enabled = true`):

| Server | Language | Auto-trigger Condition |
|--------|------|--------------|
| rust-analyzer | Rust | `Cargo.toml`, `*.rs` |
| gopls | Go | `go.mod`, `*.go` |
| clangd | C/C++ | `compile_commands.json`, `CMakeLists.txt`, `.clangd`, `*.c`, `*.cc`, `*.cpp`, `*.cxx`, `*.h`, `*.hh`, `*.hpp` |
| pyright | Python | `pyproject.toml`, `setup.py`, `requirements.txt`, `*.py` |
| typescript-language-server | TypeScript/JavaScript | `tsconfig.json`, `package.json` |
| zls | Zig | `build.zig`, `*.zig` |
| lua-language-server | Lua | `.luarc.json`, `.luarc.jsonc`, `*.lua` |

The companion columns above are `root_markers` and the server's extension list
(`crates/lsp/src/servers.rs`).

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
(`apps/endside/src/support/config.rs`).

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

Sources: `apps/serverside/src/daemon_runtime.rs`
(`resolve_allowed_tool_names`: empty map → all names; unknown names `continue`;
`false` removes, `true` inserts), mirrored TUI-side in
`apps/endside/src/gateway_api/runtime_request.rs`
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
- `Ctrl+T` - Switch reasoning effort (off/high/max), `apps/endside/src/input/event_key.rs`
- Status bar shows current agent role name

---

## Keybindings

All TUI keybindings are **hard-coded**. There is no keymap configuration table,
no config-file section for keys, and no validation layer: the readline/emacs
editing chords live in `handle_input_key`
(`apps/endside/src/input/core.rs`) and the app-level chords in
`apps/endside/src/input/event_key.rs`. The source itself states "These bindings
are hard-coded — there is no keymap configuration"
(`apps/endside/src/input/core.rs`).

### Input-line editing (readline / emacs)

| Chord | Action |
|-------|--------|
| `Ctrl+A` / `Ctrl+E` | Move to line start / line end |
| `Ctrl+B` / `Ctrl+F` | Move one character left / right |
| `Alt+B` / `Alt+F` | Move one word left / right |
| `Ctrl+D` | Delete one character forward |
| `Ctrl+W` / `Alt+D` | Delete one word backward / forward |
| `Ctrl+U` / `Ctrl+K` | Kill to line start / line end |

`Ctrl+A` is **line start**, not select-all. Keyboard selection uses
`Shift+Left`/`Shift+Right`/`Shift+Home`/`Shift+End` instead
(`apps/endside/src/input/core.rs`).

Source: `apps/endside/src/input/core.rs`.

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
| `Home` / `End` | Move the caret to the start / end of the current input line |
| `PageUp` / `PageDown` | Scroll the transcript a page |
| `Esc` | Clear an active transcript selection |

Sources: submit, soft newline, input history, copy/cut, the `Ctrl+C` /
`Ctrl+Insert` copy-vs-quit split, `Ctrl+T`, `Tab`, `Up`/`Down`,
`PageUp`/`PageDown` and `Esc` handling all live in
`apps/endside/src/input/event_key.rs`; `Home`/`End` caret movement lives in
`apps/endside/src/input/core.rs`.

#### Provider / model dialog keys (`/connect`)

While the provider/model dialog is open it owns the keyboard, so the same chords
mean something different there:

| Chord | Action |
|-------|--------|
| `Up` / `Down` | Move the highlight one row (single-row scrolling, never a page) |
| `PgUp` / `PgDn` | Page the focused pane by one screen (see the paging rules above) |
| `Home` / `End` | Jump to the first / last item of the focused pane |
| `Left` / `Right` | Switch focus to the providers / models pane |
| `Tab` | Toggle focus between the two panes |
| `Enter` | Apply the highlighted provider + model (asks for an API key when the profile needs one) |
| `Ctrl+R` | Re-fetch the highlighted provider's official model list |
| `Esc` | Close the dialog without applying anything |

Sources (all inside `handle_provider_selection_key`):
`apps/endside/src/input/event_key.rs:1323-1391` (`Esc` / `Enter` at `:1323`/`:1326`,
the paging arms at `:1364-1379`, `Ctrl+R` at `:1389`).

> The dialog opens with focus in the **providers** pane, so the first `PgDn` pages
> the provider list rather than the model list; press `Right` (or `Tab`) first to
> page models.

> The key-hint row inside the dialog has two variants: on a narrow dialog the
> compact one drops `Home`/`End` and `Enter apply` so that the row still fits and
> `Esc` is not pushed off the edge.

> `Shift+Enter` only reaches the app on terminals that report key modifiers
> (kitty keyboard / CSI u); mainstream unix terminals fold it onto the same
> `0x0D` byte as plain `Enter`. `Ctrl+Shift+C` is normally intercepted by the
> terminal emulator, which is why `Ctrl+Insert` is the reliable copy key
> (`apps/endside/src/input/event_key.rs`).

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
(`apps/endside/src/main.rs`).

---

## FAQ

### Q: What configurations does TUI support?

**A**: TUI supports:
- ✅ Common configurations it actually parses: llm, subagent, skills, trace, hooker
- ❌ `[compact]` — **not read by the TUI**; it is CLI/Daemon only (see the note above)
- ✅ TUI-specific configurations (`tui`, `tui.remote`, `lsp`, `agent`)

The TUI `Config` struct has no `compact` field
(`apps/endside/src/support/config.rs`), so a `[compact]` section in a
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
configuration surface (`apps/endside/src/input/core.rs`,
`apps/endside/src/input/event_key.rs`). Adding a keymap is a known gap,
not a configuration option.

---

## Known Gaps

- **No configurable keymap.** Keybindings cannot be changed via configuration;
  there is no keymap table and no validation layer
  (`apps/endside/src/input/core.rs`).
- **`lsp.extra_servers.extensions` silently ignores globs.** A wildcard entry
  never matches and produces no warning (`crates/lsp/src/manager.rs`).
- **Unknown tool names in `[agent.<name>.tools]` are silently skipped.** The
  config structs do not use `deny_unknown_fields`, so a typo such as
  `read = true` (instead of `file_read = true`) is accepted and ignored
  (`apps/serverside/src/daemon_runtime.rs`).

---

## Reference Links

- **Common Configuration**: [config_file_guide.md](./config_file_guide.md)
- **Agent Role Development**: [custom_agent.md](./custom_agent.md)
- **Remote TUI**: [remote_tui.md](./remote_tui.md)
- **Skills Usage**: [skill_usage.md](./skill_usage.md)
- **Plugins Configuration**: [plugins.md](./plugins.md)
- **Quick Start**: [README.md](../README.md)
