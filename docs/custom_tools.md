# Declarative Custom Tools

xiaoO can discover language-agnostic custom tools from TOML manifests.

> Declarative filesystem custom tools are disabled for E2B runtimes. Their files
> may exist in the copied workspace but are not registered; see
> [E2B Workspace 与 Skills Bootstrap](./e2b_workspace_skills_bootstrap.md#custom-tools).

## Locations

- Project tools: `<workspace>/.xiaoo/tools/*.toml`
- Global tools: `~/.xiaoo/tools/*.toml`

Only these two directories are scanned, and only `*.toml` files in them
(`crates/tool/src/impl/plugin/tool_source.rs:31-50,53-60`). The workspace directory is searched
before the global one, and the first manifest to claim a name is the one reported as `active`; a
later duplicate is marked `shadowed` and records the winning path in `shadowed_by`
(`crates/tool/src/impl/plugin/catalog.rs:295-320,375-386`). A tool whose `exec.command` cannot be
found is reported as `command_missing` rather than `active` (`catalog.rs:380-386`).

Each manifest defines one tool. Tool names must be unique across built-in and custom tools — a
duplicate name fails registry construction with `duplicate tool name in registry: <name>`
(`crates/tool/src/framework/registry/builder.rs:43-59`).

## Manifest

```toml
name = "echo_payload"
description = "Echoes the custom tool stdin payload"
timeout_ms = 5000

[output]
description = "The echoed message as JSON"

[effect]
reads_filesystem = false
writes_filesystem = false
network_access = false
side_effects = false

[input_schema]
type = "object"
required = ["message"]

[input_schema.properties.message]
type = "string"
description = "Message to echo"

[exec]
command = "sh"
args = [".xiaoo/tools/echo_payload.sh"]
stdin = "json"
stdout = "json"
```

### Manifest Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | Yes | — | Unique tool name; letters, numbers, `_` or `-` |
| `description` | Yes | — | Tool description shown to the LLM |
| `timeout_ms` | No | `30000` | Execution timeout; must be greater than `0` |
| `output` | No | — | Output metadata table (see below) |
| `effect` | No | all `false` | Side-effect declaration table (see below) |
| `input_schema` | Yes | — | JSON-schema-style table describing the arguments |
| `exec` | Yes | — | How to launch the process (see below) |

Struct definition at `crates/tool/src/impl/plugin/manifest.rs:6-15`; validation rules
(name format, non-empty `description`, non-empty `exec.command`, `timeout_ms != 0`,
`input_schema` must be a table, `exec.env` entries must be plain variable names) at
`crates/tool/src/impl/plugin/manifest.rs:96-129`.

### `[output]`

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `description` | No | `"Tool output"` | Describes the tool's return shape |

`crates/tool/src/impl/plugin/manifest.rs:18-21`; the default value comes from
`default_output_description()`. The rendered schema surfaces it as `output_description`, so
`xiaoo-daemon config custom-tools` shows it for each tool
(`crates/tool/src/impl/plugin/catalog.rs:88-100`).

### `[effect]`

Declares what the tool touches, so the runtime can reason about it. All four keys default to
`false` (`crates/tool/src/impl/plugin/manifest.rs:23-36`):

| Field | Description |
|-------|-------------|
| `reads_filesystem` | The tool reads files |
| `writes_filesystem` | The tool writes or deletes files |
| `network_access` | The tool makes network requests |
| `side_effects` | The tool has effects beyond its return value |

The section is converted into an `EffectProfile` on the tool spec
(`crates/tool/src/impl/plugin/manifest.rs:132-140`;
`crates/tool/src/impl/plugin/spec.rs:33-34,60-62`) and is consumed in two places:

- **Batch scheduling**: a tool is treated as safe to run in parallel only when it does **not** write
  the filesystem and has no side effects, but does read the filesystem or use the network
  (`crates/core/src/tool_exec.rs:57-63,275-279`). Declaring honestly matters — a tool that writes
  files but reports `writes_filesystem = false` may be run concurrently with other tools, and one
  that neither reads nor writes nor uses the network and has no side effects is always serialized.
- **Manual test harness**: `xiaoo-daemon config test-custom-tool` refuses to run a tool when any
  `[effect]` flag is `true`, unless `--allow-effects` is passed
  (`crates/tool/src/impl/plugin/catalog.rs:256-264`).

`[effect]` is **not** a permission or sandbox boundary: it does not grant, deny, or restrict a tool
call at runtime.

### `[exec]`

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `command` | Yes | — | Program to run |
| `args` | No | `[]` | Argument list |
| `stdin` | No | `"json"` | `"json"` writes the payload to stdin; `"none"` closes stdin |
| `stdout` | No | `"text"` | `"text"` returns output as text; `"json"` parses it as JSON |
| `env` | No | `[]` | Host environment variable names to forward |

`crates/tool/src/impl/plugin/manifest.rs:38-47`; the mode enums are `StdinMode` (`json` | `none`) and
`StdoutMode` (`text` | `json`) at `crates/tool/src/impl/plugin/manifest.rs:50-63`. Any other value is
rejected as an unsupported mode by the config surface
(`crates/tool/src/impl/plugin/catalog.rs:74-83`).

### Command And Argument Path Rules

`command` and each entry of `args` are expanded with the same rules before the process is spawned
(`crates/tool/src/impl/plugin/executor.rs:67-101`):

| Form | Resolution |
|------|------------|
| `~` or `~/...` | Expanded against `$HOME` (`executor.rs:67-75`) |
| `./...` or `../...` | Resolved **relative to the tool directory**, not the workspace or the CWD (`executor.rs:77-99`) |
| Anything else | Passed through unchanged, so it is looked up on `PATH` |

Note the asymmetry with the process working directory: the process runs with the **workspace root**
as its CWD (`crates/tool/src/impl/plugin/executor.rs:159`), while `./`-prefixed arguments resolve
against the tool dir. A bare `./run.sh` in `args` therefore points at
`<tool_dir>/run.sh` (for a project tool, `<workspace>/.xiaoo/tools/run.sh`). If `$HOME` is unset, a
`~`-prefixed value is left unexpanded rather than failing.

## Stdin Protocol

When `stdin = "json"`, xiaoO writes:

```json
{
  "args": { "message": "hello" },
  "context": {
    "agent_id": "default",
    "model": "model-name",
    "session_id": "optional-session-id",
    "directory": "/path/to/workspace",
    "worktree": "/path/to/workspace",
    "tool_dir": "/path/to/workspace/.xiaoo/tools"
  }
}
```

The process runs with the workspace root as its current directory
(`crates/tool/src/impl/plugin/executor.rs:157-159`). The payload is built in
`crates/tool/src/impl/plugin/executor.rs:44-63`; when `stdin = "none"` no payload is written.

## Environment

xiaoO always sets (`crates/tool/src/impl/plugin/executor.rs:160-169`):

- `XIAOO_WORKSPACE_ROOT`
- `XIAOO_TOOL_DIR`
- `XIAOO_TOOL_MANIFEST`
- `XIAOO_AGENT_ID`
- `XIAOO_SESSION_ID` when available

To forward additional host environment variables, list their names:

```toml
[exec]
command = "python3"
args = [".xiaoo/tools/query.py"]
env = ["DATABASE_URL"]
```
