## Skills Usage

Skills are prompt-based reusable instruction sets. The LLM automatically invokes registered skills via the built-in `skill` tool.

### Skill Directories (Four-Level Priority)

Skills are automatically loaded from multiple directories with the following priority (highest to lowest):

1. **Project level**: `./.xiaoo/skills/` - Project-specific skills
2. **Config level**: directories specified in `[skills].dirs` - Team/user shared skills
3. **User level**: `~/.xiaoo/skills/` - Personal skills available everywhere
4. **System level**: `/usr/lib/.xiaoo/skills/` - Built-in skills like `xiaoo-guardian`

```
Skill directory structure:
./.xiaoo/skills/           # Project level (highest priority)
├── code-review/
│   └── SKILL.md
~/.xiaoo/skills/           # User level
├── lint-runner/
│   └── SKILL.toml
/usr/lib/.xiaoo/skills/    # System level (lowest priority, built-in only)
├── xiaoo-guardian/
│   └── SKILL.md
```

Additional skill directories can be added in `~/.config/xiaoo/config.toml`:

```toml
[skills]
dirs = ["/path/to/team-skills", "/path/to/project-skills"]
```

### SKILL.md Format

```markdown
---
name: code-review
description: Review code for quality and security issues
version: "1.0"
arguments: [target]
argument-hint: "[file or directory path]"
---

Review the code at $target for:
1. Security vulnerabilities
2. Performance issues
3. Code style violations

Use grep and file_read to examine the code, then provide a structured report.
```

**Frontmatter Field Reference:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | Directory name | Skill name |
| `description` | string | Auto-extracted from body | Brief description, displayed in the skill list |
| `version` | string | — | Version number |
| `user-invocable` | bool | `true` | Whether the user can manually invoke the skill |
| `disable-model-invocation` | bool | `false` | Prevent the LLM from automatically invoking the skill |
| `context` | string | `inline` | Execution mode: `inline` (expand into conversation) or `fork` (sub-agent) |
| `arguments` | list | `[]` | Named parameter list; referenced in prompts as `$arg_name` |
| `argument-hint` | string | — | Parameter hint text |
| `paths` | list | `[]` | Conditional activation glob patterns |

> When `description` is left empty, the first non-heading paragraph is automatically extracted from the markdown body.

### Management Commands

```bash
# List installed skills
xiaoo skill list

# Show skill details and prompt content
xiaoo skill show <name>

# Run a security audit on a skill directory
xiaoo skill audit <path>

# Install from a local directory (auto-audit)
xiaoo skill install ./my-skill/

# Install from a Git repository
xiaoo skill install https://github.com/user/my-skill.git

# Remove an installed skill
xiaoo skill remove <name>
```

### Built-in Skills

Builtin skills are automatically installed when you run `cargo install --path apps/xiaoo-app`. They provide security policy enforcement and other built-in capabilities, and are loaded with highest priority by the runtime.

**Installation locations** (automatic fallback):
- **System level** (preferred): `/usr/lib/.xiaoo/skills/` - requires root privileges
- **User level** (fallback): `~/.xiaoo/skills/` - used if system-level installation fails

**Builtin skills** (located in `<xiaoO>/plugins/skills/`):
- `xiaoo-guardian` - Security policy enforcement
- `block-analyzer` - Block analysis capabilities

> **Note**: `cargo build` does NOT install skills. Only `cargo install` triggers skill installation.
>
> **Installation Behavior**:
> - First attempts to install all builtin skills to system-level directory (requires root privileges)
> - If system-level installation fails (e.g., permission denied), automatically falls back to user-level directory
> - Without these skills, security features and other capabilities may be unavailable.
>
> **For system-wide installation** (recommended for multi-user environments):
> - Run `cargo install` with root privileges: `sudo cargo install --path apps/xiaoo-app`

To remove builtin skills:

```bash
# System level (requires root)
sudo rm -rf /usr/lib/.xiaoo/skills/xiaoo-guardian
sudo rm -rf /usr/lib/.xiaoo/skills/block-analyzer

# User level
rm -rf ~/.xiaoo/skills/xiaoo-guardian
rm -rf ~/.xiaoo/skills/block-analyzer
```

To completely uninstall all skills along with the application:

```bash
cargo uninstall xiaoo-app
sudo rm -rf /usr/lib/.xiaoo/skills
rm -rf ~/.xiaoo/skills
```

### Security Audit

A security audit is automatically performed before installation, checking for:

- Symbolic links
- Script files (`.sh` / `.bash`, etc., unless `allow_scripts = true` is configured)
- High-risk command patterns (`rm -rf /`, `sudo`, `curl | sh`, etc.)
- Shell chaining operators (`&&`, `||`, `;`)
- Oversized files

### Runtime Behavior

During agent runtime, loaded skills appear in the system prompt. The LLM can invoke them via the `skill` tool:

```
User: Review src/main.rs for me
LLM → calls skill tool: { skill: "code-review", args: "src/main.rs" }
     → skill prompt is expanded ($target → src/main.rs)
     → LLM performs the review using tools such as grep/file_read per the prompt
```