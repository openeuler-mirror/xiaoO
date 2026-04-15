## Skills Usage

Skills are prompt-based reusable instruction sets. The LLM automatically invokes registered skills via the built-in `skill` tool.

### Skill Directories

Skills are automatically loaded from `~/.xiaoo/skills/` by default. Each subdirectory containing a `SKILL.md` or `SKILL.toml` constitutes a skill:

```
~/.xiaoo/skills/
├── code-review/
│   └── SKILL.md
├── lint-runner/
│   └── SKILL.toml
└── ...
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