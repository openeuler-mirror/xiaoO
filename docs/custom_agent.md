# Custom Agent Development Guide

> **Note**: This document focuses on Agent role development (TUI/Daemon multi-role switching).
> For Subagent role configuration, please refer to the `[subagent]` section in [Configuration File Guide](./config_file_guide.md).

This document describes how to configure custom Agents in xiaoo. An Agent role is declared entirely through **TOML configuration** in `~/.config/xiaoo/config.toml`. Slash commands (`~/.xiaoo/commands/<name>.md`) are a separate feature — they are standalone prompt templates invoked from the TUI input box, not agent role definitions — but a slash command can be used to drive an agent by invoking a skill or by pre-expanding a prompt body before the turn is submitted.

---

## 1. Agent Configuration Architecture

```
Agent Role Configuration
└── TOML Config (~/.config/xiaoo/config.toml) [REQUIRED]
    ├── Description
    ├── System Prompt (prompt)
    ├── Max turns (optional)
    └── Tool Permission Management (fine-grained control) [OPTIONAL]
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

`[tui].agent_order` is optional. It controls the full Agent tab order. Include `Core` to place the default agent anywhere in the tab bar; if `Core` is omitted, it stays first for backward compatibility. Roles omitted from the list are appended in the default alphabetical order. The builtin `plan` role is always appended last if not explicitly ordered.

### 2.3 Configuration Reference

#### Basic Configuration (`[agent.<name>]`)

| Field | Required | Description |
|-------|----------|-------------|
| `description` | Yes | Agent description displayed in help output |
| `prompt` | Yes | System Prompt defining the Agent's behavior |
| `max_turns` | No | Optional cap on agent loop turns for this role |

> The `plan` role is builtin (see `apps/shared/src/builtin_agent_roles.rs`).
> Declaring `[agent.plan]` in config is rejected at startup.

#### Tool Permissions (`[agent.<name>.tools]`)

| Setting | Description |
|---------|-------------|
| `tool_name = true` | Explicitly allow the tool |
| `tool_name = false` | Explicitly deny the tool |
| Not defined | Falls back to global permission policy |

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

> The command loader only parses `description` from the frontmatter.
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
- [ ] `description` and `prompt` are both set
- [ ] Tool permissions in TOML follow the principle of least privilege
- [ ] The Agent loads successfully (use Tab to switch agent in TUI)
- [ ] You did **not** declare `[agent.plan]` (it is builtin and cannot be overridden)
