# Custom Agent Development Guide

> **Note**: This document focuses on Agent role development (TUI/Daemon multi-role switching).
> For Subagent role configuration, please refer to the `[subagent]` section in [Configuration File Guide](./config_file_guide.md).

This document describes how to configure custom Agents in xiaoo. An Agent role is declared entirely through **TOML configuration** in `~/.config/xiaoo/config.toml`. Slash commands (`~/.xiaoo/commands/<name>.md`) are a separate feature — they are standalone prompt templates invoked from the TUI input box, not agent role definitions — but a slash command can be used to drive an agent by invoking a skill or by pre-expanding a prompt body before the turn is submitted.

---

## 1. Agent Configuration Architecture

```
Agent Role Configuration
└── TOML Config (~/.config/xiaoo/config.toml) [REQUIRED]
    ├── Description (optional)
    ├── System Prompt (prompt, optional)
    ├── Max turns (optional)
    └── Tool visibility mask (optional)
```

**How it works**: On startup, the TUI/Daemon reads all `[agent.<name>]` entries from `config.toml` into the agent role registry. The builtin `plan` role is injected automatically and cannot be overridden. The `Core` tab (the default agent with no role-specific prompt) is always present and represents the absence of an active role.

Slash commands under `~/.xiaoo/commands/` are loaded separately by `apps/endside/src/services/command_loader.rs` and are surfaced as `/command-name` autocompletions; they are not merged into agent roles.

---

## 2. TOML Configuration

TOML serves as the **registration entry point** for every Agent role. An Agent must be declared in TOML to be recognized by the runtime.

### 2.1 File Location

Edit `~/.config/xiaoo/config.toml`

### 2.2 Configuration Structure

Edit `[agent.<name>]` and `[agent.<name>.tools]`

Example:
```toml
[tui]
agent_order = ["Agent1", "Core", "Agent2", "plan"]

[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer. Focus on security, performance, and maintainability."

[agent.code-reviewer.tools]
file_write = false
file_edit = false
```

`[tui].agent_order` is optional. It controls the full Agent tab order. Include `Core` to place the default agent anywhere in the tab bar; if `Core` is omitted, it stays first for backward compatibility. Roles omitted from the list are appended in the default alphabetical order. The builtin `plan` role is always appended last if not explicitly ordered (`apps/endside/src/state/app_state.rs:1325-1375`; role names in the list are matched case-insensitively).

Applying the example above with the mask rules from §2.3 means `code-reviewer` sees every tool except
`file_write` and `file_edit`; `bash` stays visible because it is not listed at all.

### 2.3 Configuration Reference

#### Basic Configuration (`[agent.<name>]`)

| Field | Required | Description |
|-------|----------|-------------|
| `description` | No | Agent description displayed in help output; defaults to an empty string |
| `prompt` | No | System Prompt defining the Agent's behavior; when omitted the global system prompt is used |
| `max_turns` | No | Optional cap on agent loop turns for this role |

Both `description` and `prompt` are optional: `description` is `String` with `#[serde(default)]` and
`prompt` is an `Option<String>` (`apps/serverside/src/daemon_config.rs:292-301`,
`apps/endside/src/support/config.rs:158-167`). Omitting `prompt` is not the same as setting it to
the empty string — an empty or whitespace-only prompt is filtered out and the role falls back to the
global `agent.system_prompt` (`apps/serverside/src/daemon_runtime.rs:766-770`; the TUI path uses
`state.active_agent_role_config()` and falls back to the default system prompt at
`apps/endside/src/gateway_api/runtime_request.rs:227-233`). Note also that the daemon-side
`description` field is currently marked `#[allow(dead_code)]`, so it is accepted but not consumed by
the daemon runtime (`apps/serverside/src/daemon_config.rs:293-295`).

> The `plan` role is builtin (see `apps/shared/src/builtin_agent_roles.rs`).
> Declaring `[agent.plan]` in config is rejected at startup
> (`apps/serverside/src/daemon_config.rs:1041-1043`).
> Its own tool mask is defined in code and hides `bash`, `file_edit`, `file_write`, `send_file` and
> `spawn_subagent` (`apps/serverside/src/daemon_config.rs:1046-1062`) — it is a read-only planning
> role, and the same visibility-mask rules apply to it.

#### Tool Visibility (`[agent.<name>.tools]`)

`[agent.<name>.tools]` is a **visibility mask**, not a permission policy. It decides which tools
appear in the agent's tool list; it does not grant or revoke capabilities, and it has no fallback to
any global permission policy.

| Setting | Effect |
|---------|--------|
| `tool_name = true` | The tool is explicitly made visible |
| `tool_name = false` | The tool is **removed** from the visible set |
| Not defined | The tool **stays visible** (tools are visible by default) |
| No `[agent.<name>.tools]` table, or an empty table | **All** tools are visible |

Resolution is a mask over the discovered tool set
(`apps/serverside/src/daemon_runtime.rs:969-999`;
`apps/endside/src/gateway_api/runtime_request.rs:429-458`):

1. Start from all discovered tool names.
2. If there is no agent role, or its `tools` map is empty, return every tool unchanged
   (`daemon_runtime.rs:973-978`; `runtime_request.rs:430-433`).
3. Otherwise start from the full set and apply each entry: `true` inserts the name, `false` removes
   it (`daemon_runtime.rs:980-992`; `runtime_request.rs:435-456`).
4. Only the *visible* tools are registered for the agent; filtering happens at registry build time
   (`crates/tool/src/framework/registry/registry_impl.rs:42-63`).

**Name mismatches are silently skipped.** If a configured key is not in the discovered set, that
entry is ignored with no warning and no error (`daemon_runtime.rs:984-986`;
`runtime_request.rs:447-449`). A typo such as `read = false` therefore disables nothing at all,
because no tool is named `read` — the real name is `file_read`. Use the real tool names listed
below, and verify with `xiaoo-daemon config tools`.

Because it is only a visibility mask, removing `bash` / `file_write` does not stop a visible tool
from having those effects through another path; treat the mask as prompt-surface hygiene and pair it
with hooker-based enforcement (see [plugins.md](./plugins.md)) when you need a hard guarantee.

Real built-in tool names (from `crates/tool/src/impl/builtin/*/spec.rs`):
`ask_user_question`, `bash`, `count_text_length`, `file_edit`, `file_read`, `file_write`, `glob`,
`grep`, `join_subagent`, `lsp`, `print_hello_world`, `send_file`, `skill`, `spawn_subagent`,
`todo_write`, `webfetch`, `web_search`.

> **Known gap**: `[subagent.<role>.tools]` is parsed and carried into the session as a
> `SubagentRoleRecord` (`apps/serverside/src/daemon_runtime.rs:771-790`;
> `apps/endside/src/gateway_api/runtime_request.rs:265-281`), but the delegation path only reads the
> role's `prompt`, `max_turns` and `description` — the role's `tools` map is never applied to the
> child's tool list (`apps/shared/src/gateway/session_supervisor.rs:345-356`). A custom-role
> subagent therefore currently receives the full toolset, unlike the *default* exploration subagent,
> which is narrowed by a separate hardcoded allowlist to `file_read`, `glob` and `grep`
> (`apps/shared/src/gateway/hosted_runtime_resolver.rs:40-47,141-150,170-175`). Do not rely on
> `[subagent.<role>.tools]` to restrict a subagent.
>
> The `[subagent]` section is otherwise out of scope for this document; see the `[subagent]` section
> of [config_file_guide.md](./config_file_guide.md).

---

## 3. Slash Command Files (Separate Feature)

Slash commands are reusable prompt templates invoked from the TUI input box
with `/<name>`. They live in `~/.xiaoo/commands/<name>.md` and are loaded by
`apps/endside/src/services/command_loader.rs`. They are **not** agent role
definitions and are not linked to `[agent.<name>]` entries — a command file
just provides a prompt body that gets submitted as the user turn (optionally
with leading `/command args` parsing).

### 3.1 File Location

Create a Markdown file in `~/.xiaoo/commands/`:

```
~/.xiaoo/commands/
├── bugfix.md
├── deploy.md
└── review.md
```

The command name is the filename stem (without `.md`). Invoke it in the TUI
with `/<name>`, optionally followed by arguments.

### 3.2 File Format

```markdown
---
description: Automated bugfix workflow for AET
---

Automatically detect the language of user input and respond in the same language.

Invoke the bugfix-automation skill with the provided natural language arguments, then execute exactly as the skill presents.
```

### 3.3 Format Breakdown

| Section | Description |
|---------|-------------|
| Frontmatter wrapped in `---` | Optional YAML-like metadata; only `description` is read by the loader |
| Body after closing `---` | Prompt body injected as the user turn when the slash command is invoked |

### 3.4 Frontmatter Fields

| Field | Required | Description |
|-------|----------|-------------|
| `description` | No | Short summary shown in slash autocompletion; empty string if omitted |

> The command loader only parses `description` from the frontmatter — it reads that one key and
> ignores the rest (`apps/endside/src/services/command_loader.rs:56-63`).
> `disable-model-invocation` is a **skill** frontmatter field (see
> [skill_usage.md](./skill_usage.md)), not a command-file field — setting it in
> a command file has no effect on agent behavior.

### 3.5 Relationship to Agents

A slash command does not switch the active agent role. To combine them:

1. Switch to the desired agent role with `Tab` (or via `[tui].agent_order`).
2. Invoke the slash command — its body is submitted as the user turn for the
   currently active agent role.

The `*.Chat.command.before` hooker (see [plugins.md](./plugins.md)) can
inspect or transform the command body before it becomes a user turn.

---

## 4. Configuration Checklist

After creating an Agent role, verify the following:

- [ ] `config.toml` contains an `[agent.<name>]` declaration (**required**)
- [ ] `description` and `prompt` are set if you need a role-specific prompt (both are optional)
- [ ] Every key in `[agent.<name>.tools]` is a real built-in tool name; unknown names are ignored
      silently, so a typo silently hides nothing (`xiaoo-daemon config tools` lists the real names)
- [ ] You understand that `.tools` is a visibility mask, not a permission boundary
- [ ] The Agent loads successfully (use Tab to switch agent in TUI)
- [ ] You did **not** declare `[agent.plan]` (it is builtin and cannot be overridden)
