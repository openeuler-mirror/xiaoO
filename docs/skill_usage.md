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
(`crates/skill/src/loading/loader.rs`; directory assembly in
`apps/endside/src/cli/skills.rs`). This is why a project-local skill can shadow a
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
| `when-to-use` / `when_to_use` | string | — | Scenario hint: when this skill should be used. Shown in dynamic discovery hints (see [Skill Discovery Hints](#skill-discovery-hints-l2)); keep it disjoint from `description` |
| `version` | string | — | Version number |
| `author` | string | — | Skill author or owning team |
| `tags` | list | `[]` | Free-form labels; also drive dynamic discovery hints — matched against user-message words (word-boundary, case-insensitive) and against edited file extensions (e.g. tag `rust` matches `.rs` files) |
| `user-invocable` / `user_invocable` | bool | `true` | Whether the user can manually invoke the skill |
| `disable-model-invocation` / `disable_model_invocation` | bool | `false` | Prevent the LLM from automatically invoking the skill |
| `context` | string | `inline` | Execution mode: `inline` (expand into conversation) or `fork` (sub-agent) |
| `arguments` | list | `[]` | Named parameter list; referenced in prompts as `$arg_name` |
| `argument-hint` / `argument_hint` | string | — | Parameter hint text |
| `paths` | list | `[]` | Conditional activation glob patterns; also drive write-pivot discovery (see [Skill Discovery Hints](#skill-discovery-hints-l2)) |

Frontmatter fields (including `author` and `tags`) are declared and parsed in
`crates/skill/src/loading/md_parser.rs`. `name` falls back to the containing directory name when omitted
(`md_parser.rs`). Note that `user-invocable`, `disable-model-invocation`,
`when-to-use` and `argument-hint` each accept **both** the kebab-case and the snake_case spelling. An unrecognized
`context` value is silently ignored (falls back to no explicit context) rather than being an
error (`md_parser.rs`).

> When `description` is left empty, the first non-heading paragraph is automatically extracted
> from the markdown body (`crates/skill/src/loading/md_parser.rs`).

### Skill Discovery Hints (L2)

Besides the always-visible *Available Skills* listing in the system prompt
(L1), the agent injects **dynamic discovery hints** into the conversation when
a skill becomes relevant to the current activity
(`crates/core/src/skill_discovery.rs`). A hint is a `<system-reminder>` user
message listing skill name + `when_to_use` + the trigger reason — it never
repeats `description` (that belongs to the L1 listing) or the skill body
(that belongs to an actual `skill` invocation).

Two signals trigger a hint (both local and synchronous; no semantic search):

1. **User input signal** — a user message containing one of the skill's
   `tags` as a whole word (case-insensitive; word boundaries are ASCII word
   characters, so `parsers` does **not** match tag `rs`, while
   `帮我看下rs文件` does).
2. **Write pivot** — a completed `file_write` / `file_edit` whose target path
   matches a `paths` glob, or whose file extension matches a `tags` entry
   (bare extension or common language alias: `.rs` matches tags `rs`/`rust`;
   `.py` matches `py`/`python`; `.md` matches `md`/`markdown`; likewise
   `js`/`javascript`, `ts`/`typescript`, `go`/`golang`, `sh`/`bash`/`shell`,
   `cpp`/`c++`, `yml`/`yaml`, `rb`/`ruby`, `kt`/`kotlin`).

Lifecycle rules:

- Each skill is hinted **at most once** per accounting period. A successful
  `skill` tool invocation or a `/skill-name` expansion also counts as "already
  visible" (the full text is in history).
- Context compression clears the ledger: still-relevant skills may be
  re-discovered afterwards.
- Skills with `disable-model-invocation: true` are never hinted (the model
  cannot invoke them).
- Hints are gated by the internal `FeatureFlags` fields `skill_matching`
  and `skill_discovery` (both default on, and L2 additionally requires L1).
  **Known gap — neither flag is file-configurable.** No `[skills]` (or any
  other) config key maps to them, and every shipped deployment constructs
  `FeatureFlags` with `..Default::default()`
  (`apps/serverside/src/daemon_runtime.rs`,
  `apps/endside/src/gateway_api/runtime_request.rs`,
  `apps/endside/src/cli/entry.rs`), so both layers are effectively always
  on; `skill_discovery` can only be turned off programmatically by an
  embedder that constructs its own `FeatureFlags` around the core runtime.

### Disabling And Audit Configuration

`[skills]` accepts the following keys (`apps/shared/src/skills_support.rs`;
the shared `SkillsSection` schema is re-exported and parsed identically by the
daemon, TUI, and CLI):

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `dirs` | list | `[]` | Extra skill directories inserted at config level |
| `allow_scripts` | bool | `false` | Allow script files in a skill without failing the audit |
| `disabled` | list | `[]` | Skill names to skip at load time |
| `max-listing-description-chars` | int | `250` | Truncation cap (Unicode scalars) for descriptions in the system prompt's *Available Skills* listing; management APIs / `skill list` / `skill show` keep serving full text |

```toml
[skills]
dirs = ["/path/to/team-skills"]
allow_scripts = false
disabled = ["block-analyzer"]
```

A skill listed in `disabled` is skipped by name during loading
(`crates/skill/src/loading/loader.rs`); the daemon still *discovers* it so it can report
it as disabled rather than missing (`apps/serverside/src/skill_management.rs`).

Audit on load is controlled by the `audit_enabled` field on the internal `SkillsConfig`, which
**defaults to `false`** (`crates/skill/src/types/config.rs`). When enabled, a skill
directory failing the audit is skipped with a warning
(`crates/skill/src/loading/loader.rs`).

> **Known gap**: `audit_enabled`, `prompt_injection_mode` and `prompt_budget_ratio`
> exist only on the internal `SkillsConfig`
> (`crates/skill/src/types/config.rs`) and are **not** exposed in the config file schema:
> `SkillsSection` declares only `dirs` / `allow_scripts` / `disabled` /
> `max-listing-description-chars`
> (`apps/shared/src/skills_support.rs`), and every construction site fills the rest
> from `SkillsConfig::default()` (`apps/serverside/src/daemon_config.rs`,
> `apps/endside/src/support/config.rs`, `apps/endside/src/cli/skills.rs`). Because
> the config structs do not use `deny_unknown_fields`, writing these keys into `config.toml`
> **passes validation silently but has no effect**. Enabling the load-time
> audit requires an API consumer to set the field programmatically.
>
> `max_listing_description_chars` **is** file-configurable via
> `[skills] max-listing-description-chars` (default `250`) and consumed by
> `FileSkillRegistry::list_skills`, which truncates skill descriptions in the
> system prompt's *Available Skills* listing to keep the section byte-stable
> and bounded; management APIs, `skill list`, and `skill show` keep serving
> full text via the spec accessors. `prompt_injection_mode` and
> `prompt_budget_ratio` remain inert — the full/compact injection modes and the
> prompt budget ratio have no reader anywhere in the workspace (the
> *Available Skills* listing and the L2 discovery hints are the paths that
> actually reach the prompt, see
> [Skill Discovery Hints](#skill-discovery-hints-l2)).

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

The subcommands above are defined in `apps/endside/src/cli/skills.rs`.
`skill install` copies into a **user-level** destination by default — system level
(`/usr/lib/.xiaoo/skills`) is reserved for built-in skills.

### Built-in Skills

Builtin skills are automatically installed when you run `cargo install --path apps/endside`
(the install step is a build script gated on `is_cargo_install()`:
`apps/shared/build.rs`, source directory `plugins/skills` at `apps/shared/build.rs`).
They provide security policy enforcement and other built-in capabilities.

> **Priority note**: builtins live at the system level, which is the *lowest* priority directory
> (`crates/skill/src/loading/loader.rs`; `apps/endside/src/cli/skills.rs`). A skill
> of the same name in the project, a config dir, or the user directory takes precedence, and the
> builtin is then skipped as a duplicate.

**Installation locations** (automatic fallback):
- **System level** (preferred): `/usr/lib/.xiaoo/skills/` - requires root privileges
- **User level** (fallback): `~/.xiaoo/skills/` - used if system-level installation fails

Fallback logic: system-level install is attempted first, and only the skills that failed there are
retried at user level (`apps/shared/build.rs`).

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
- Shell chaining operators (`&&`, `||`, `;`, `|`, and backticks)
- Oversized files

> **Note**: The audit is **not** run automatically during `skill install`. The install command
> records this explicitly — "Audit is currently disabled by default; use
> `xiaoo skill audit <path>` for manual checks." (`apps/endside/src/cli/skills.rs`). Run the
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
