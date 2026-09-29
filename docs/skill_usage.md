## Skills Usage

Skills are prompt-based reusable instruction sets. The LLM automatically invokes registered skills via the built-in `skill` tool.

### Skill Directories (Four-Level Priority)

Skills are automatically loaded from multiple directories. The scan order is:

1. **Project level**: `./.xiaoo/skills/` - Project-specific skills
2. **Config level**: directories specified in `[skills].dirs` - Team/user shared skills
3. **User level**: `~/.xiaoo/skills/` - Personal skills available everywhere
4. **System level**: `/usr/lib/.xiaoo/skills/` - Built-in skills like `xiaoo-guardian`

The order above is also the **precedence** order: project level has the highest priority and
system level the lowest. Deduplication is first-wins by directory order, so a skill whose name
already appeared in an earlier directory is skipped
(`crates/skill/src/loading/loader.rs:14-60`; directory assembly in
`apps/endside/src/cli/skills.rs:24-53`). This is why a project-local skill can shadow a
built-in one of the same name — the built-in does **not** win.

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
author: platform-team
tags: [review, security]
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
| `author` | string | — | Skill author or owning team |
| `tags` | list | `[]` | Free-form labels for grouping and search |
| `user-invocable` / `user_invocable` | bool | `true` | Whether the user can manually invoke the skill |
| `disable-model-invocation` / `disable_model_invocation` | bool | `false` | Prevent the LLM from automatically invoking the skill |
| `context` | string | `inline` | Execution mode: `inline` (expand into conversation) or `fork` (sub-agent) |
| `arguments` | list | `[]` | Named parameter list; referenced in prompts as `$arg_name` |
| `argument-hint` / `argument_hint` | string | — | Parameter hint text |
| `paths` | list | `[]` | Conditional activation glob patterns |

Field parsing in `crates/skill/src/loading/md_parser.rs:93-151`; `author` and `tags` are read at
`:98-99` and `:132-135`. `name` falls back to the containing directory name when omitted
(`md_parser.rs:31-41`). Note that `user-invocable`, `disable-model-invocation` and
`argument-hint` each accept **both** the kebab-case and the snake_case spelling. An unrecognized
`context` value is silently ignored (falls back to no explicit context) rather than being an
error (`md_parser.rs:144-150`).

> When `description` is left empty, the first non-heading paragraph is automatically extracted
> from the markdown body (`crates/skill/src/loading/md_parser.rs:156-173`).

### Disabling And Audit Configuration

`[skills]` accepts the following keys (`apps/serverside/src/daemon_config.rs:261-268`,
`apps/endside/src/support/config.rs:182-189`):

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `dirs` | list | `[]` | Extra skill directories inserted at config level |
| `allow_scripts` | bool | `false` | Allow script files in a skill without failing the audit |
| `disabled` | list | `[]` | Skill names to skip at load time |

```toml
[skills]
dirs = ["/path/to/team-skills"]
allow_scripts = false
disabled = ["block-analyzer"]
```

A skill listed in `disabled` is skipped by name during loading
(`crates/skill/src/loading/loader.rs:59-67`); the daemon still *discovers* it so it can report
it as disabled rather than missing (`apps/serverside/src/skill_management.rs:41-44`).

Audit on load is controlled by the `audit_enabled` field on the internal `SkillsConfig`, which
**defaults to `false`** (`crates/skill/src/types/config.rs:10,22`). When enabled, a skill
directory failing the audit is skipped with a warning
(`crates/skill/src/loading/loader.rs:46-57`).

> **Known gap**: `audit_enabled`, `prompt_injection_mode`, `prompt_budget_ratio` and
> `max_listing_description_chars` exist only on the internal `SkillsConfig`
> (`crates/skill/src/types/config.rs:8-13`) and are **not** exposed in the config file schema:
> `SkillsSection` declares only `dirs` / `allow_scripts` / `disabled`
> (`apps/serverside/src/daemon_config.rs:261-268`), and every construction site fills the rest
> from `SkillsConfig::default()` (`apps/serverside/src/daemon_config.rs:884-898`,
> `apps/endside/src/support/config.rs:350-362`, `apps/endside/src/cli/skills.rs:55-67`). Because
> the config structs do not use `deny_unknown_fields`, writing these keys into `config.toml`
> **passes validation silently but has no effect**. As of this document, enabling the load-time
> audit requires an API consumer to set the field programmatically.
>
> **Known gap**: `prompt_injection_mode`, `prompt_budget_ratio` and
> `max_listing_description_chars` have no reader anywhere in the workspace — a search finds only
> their declarations and defaults in `crates/skill/src/types/config.rs:11-13,23-25` — so the
> `full`/`compact` injection modes, the prompt budget ratio and the description-length cap are
> currently inert. Skill listings reach the prompt through the skills *directory* table instead
> (`apps/shared/src/gateway/prompt_utils.rs:33-70`).

### Management Commands

```bash
# List installed skills
xiaoo --cli skill list

# Show skill details and prompt content
xiaoo --cli skill show <name>

# Run a security audit on a skill directory
xiaoo --cli skill audit <path>

# Install from a local directory
xiaoo --cli skill install ./my-skill/

# Install from a Git repository
xiaoo --cli skill install https://github.com/user/my-skill.git

# Remove an installed skill
xiaoo --cli skill remove <name>
```

The subcommands above are defined in `apps/endside/src/cli/skills.rs:9-20`.
`skill install` copies into a **user-level** destination by default (`:351`) — system level
(`/usr/lib/.xiaoo/skills`) is reserved for built-in skills.

### Built-in Skills

Builtin skills are automatically installed when you run `cargo install --path apps/endside`
(the install step is a build script gated on `is_cargo_install()`:
`apps/shared/build.rs:6-9`, source directory `plugins/skills` at `apps/shared/build.rs:55-63`).
They provide security policy enforcement and other built-in capabilities.

> **Priority note**: builtins live at the system level, which is the *lowest* priority directory
> (`crates/skill/src/loading/loader.rs:14-60`; `apps/endside/src/cli/skills.rs:47-48`). A skill
> of the same name in the project, a config dir, or the user directory takes precedence, and the
> builtin is then skipped as a duplicate.

**Installation locations** (automatic fallback):
- **System level** (preferred): `/usr/lib/.xiaoo/skills/` - requires root privileges
- **User level** (fallback): `~/.xiaoo/skills/` - used if system-level installation fails

Fallback logic: system-level install is attempted first, and only the skills that failed there are
retried at user level (`apps/shared/build.rs:99-152`).

**Builtin skills** (located in `<xiaoO>/plugins/skills/`):
- `xiaoo-guardian` - Security policy enforcement
- `block-analyzer` - Block analysis capabilities
- `security-rules-tester` - Security rule test fixtures

> **Note**: `cargo build` does NOT install skills. Only `cargo install` triggers skill installation.
>
> **Installation Behavior**:
> - First attempts to install all builtin skills to system-level directory (requires root privileges)
> - If system-level installation fails (e.g., permission denied), automatically falls back to user-level directory
> - Without these skills, security features and other capabilities may be unavailable.
>
> **For system-wide installation** (recommended for multi-user environments):
> - Run `cargo install` with root privileges: `sudo cargo install --path apps/endside`

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
cargo uninstall xiaoo-endside
sudo rm -rf /usr/lib/.xiaoo/skills
rm -rf ~/.xiaoo/skills
```

### Security Audit

A security audit can be run manually on a skill directory before installation. The audit checks for:

- Symbolic links
- Script files (`.sh` / `.bash`, etc., unless `allow_scripts = true` is configured)
- High-risk command patterns (`rm -rf /`, `sudo`, `curl | sh`, etc.)
- Shell chaining operators (`&&`, `||`, `;`)
- Oversized files

> **Note**: The audit is **not** run automatically during `skill install`. The install command
> records this explicitly — "Audit is currently disabled by default; use
> `xiaoo skill audit <path>` for manual checks." (`apps/endside/src/cli/skills.rs:353`). Run the
> audit manually first with `xiaoo --cli skill audit <path>` if you want to review a skill before
> installing it.

### Runtime Behavior

During agent runtime, loaded skills appear in the system prompt. The LLM can invoke them via the `skill` tool:

```
User: Review src/main.rs for me
LLM → calls skill tool: { skill: "code-review", args: "src/main.rs" }
     → skill prompt is expanded ($target → src/main.rs)
     → LLM performs the review using tools such as grep/file_read per the prompt
```
