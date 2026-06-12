# Declarative Custom Tools

xiaoO can discover language-agnostic custom tools from TOML manifests.

## Locations

- Project tools: `<workspace>/.xiaoo/tools/*.toml`
- Global tools: `~/.xiaoo/tools/*.toml`

Each manifest defines one tool. Tool names must be unique across built-in and custom tools.

## Manifest

```toml
name = "echo_payload"
description = "Echoes the custom tool stdin payload"
timeout_ms = 5000

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
stdout = "text"
```

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

The process runs with the workspace root as its current directory.

## Environment

xiaoO always sets:

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
