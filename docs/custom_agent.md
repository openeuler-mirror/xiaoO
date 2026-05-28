# Custom Agent Development Guide

> **Note**: This document focuses on Agent role development (TUI/Daemon multi-role switching).
> For Subagent role configuration, please refer to the `[subagent]` section in [Configuration File Guide](./config_file_guide.md).

This document describes how to configure custom Agents in xiaoo. An Agent configuration consists of two parts: **TOML configuration** (required, defines system prompt and permission control) and **command** (optional, defines custom quick command).

---

## 1. Agent Configuration Architecture

```
Agent Full Configuration
├── TOML Config (~/.config/xiaoo/config.toml) [REQUIRED]
│   ├── Description
│   ├── System Prompt (prompt)
│   └── Tool Permission Management (fine-grained control) [OPTIONAL]
│
└── Command File (~/.xiaoo/command/<agent-name>)
    ├── Frontmatter metadata (supplementary settings, e.g. disable-model-invocation)
    └── Instruction body (System Prompt, can be overridden by TOML)
```
**How it works**: On startup, Gateway reads all `[agent.<name>]` entries from `config.toml` as the Agent registry. It then scans the `~/.xiaoo/command/` directory and merges any matching command files.

---

## 2. TOML Configuration

TOML serves as the **registration entry point** for every Agent. An Agent must be declared in TOML to be recognized by Gateway.

### 2.1 File Location

Edit `~/.config/xiaoo/config.toml`

### 2.2 Configuration Structure

Edit `[agent.<name>]` and `[agent.<name>.tools]`

Example:
```toml
[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer. Focus on security, performance, and maintainability."

[agent.code-reviewer.tools]
file_write = false
file_edit = false
```

### 2.3 Configuration Reference

#### Basic Configuration (`[agent.<name>]`)

| Field | Required | Description |
|-------|----------|-------------|
| `description` | Must | Agent description displayed in help output |
| `prompt` | Must | System Prompt defining the Agent's behavior |

#### Tool Permissions (`[agent.<name>.tools]`)

| Setting | Description |
|---------|-------------|
| `tool_name = true` | Explicitly allow the tool |
| `tool_name = false` | Explicitly deny the tool |
| Not defined | Falls back to global permission policy |

---

## 3. Command File (Optional Supplement)

Use a command file when you need to:
- Separate a long System Prompt from TOML for better maintainability
- Set command-file-only properties such as `disable-model-invocation`

### 3.1 File Location

Create a file named after the Agent in `~/.xiaoo/command/` (no file extension required).

> The filename **must** exactly match the `<name>` in `[agent.<name>]` from TOML.

### 3.2 File Format

```markdown
---
description: Automated bugfix workflow for AET
disable-model-invocation: true
---

Automatically detect the language of user input and respond in the same language.

Invoke the bugfix-automation skill with the provided natural language arguments, then execute exactly as the skill presents.
```

### 3.3 Format Breakdown

| Section | Description |
|---------|-------------|
| Frontmatter wrapped in `---` | Metadata area for supplementary properties |
| Body after closing `---` | System Prompt instructions (can be overridden by TOML `prompt`) |

### 3.4 Frontmatter Fields

| Field | Description |
|-------|-------------|
| `description` | Agent description (overridden by TOML if both are set) |
| `disable-model-invocation` | Set to `true` to disable model calls (tool-only execution). **Can only be set in the command file.** |

Example:
```markdown
---
disable-model-invocation: true
---

Automatically detect the language of user input and respond in the same language.
Invoke the bugfix-automation skill with the provided arguments, then execute exactly as the skill presents.
```

> TOML defines description and tool permissions; the command file provides detailed instructions and enables `disable-model-invocation`.


## 4. Configuration Checklist

After creating an Agent, verify the following:

- [ ] `config.toml` contains an `[agent.<name>]` declaration (**required**)
- [ ] Tool permissions in TOML follow the principle of least privilege
- [ ] The Agent loads successfully (use Tab to switch agent in TUI)