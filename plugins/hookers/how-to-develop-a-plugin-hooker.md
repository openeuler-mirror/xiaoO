# How to develop a plugin hooker

This guide explains how to create a plugin hooker without touching Rust code in the `hooker` crate.

## 0. The simplest way

Add your hooker under `<your_xiaoO>/plugins/hookers`. A subdirectory is recognized as a valid hooker only if it contains a `plugin.json` (refers to plugin.json-example). For file configuration details, see the sections below. If your hooker needs extra setup, add an `install.sh` in the hooker directory — it will be executed automatically. After adding a hooker, run `<your_xiaoO>/plugins/hookers/install.sh` manually and follow the prompts to configure.

## 1. What is a plugin hooker?

A plugin hooker is defined by JSON and executed by an external command.

That command can be:

- a Python script
- a shell script
- a compiled binary
- any other executable command available on the machine

## 2. How plugin hookers are loaded

Boot config uses `HookerRegistryConfig.plugins`.

This is a list of JSON file paths.

Each file:

- represents one plugin file source, often owned by one developer or one feature area
- must contain a JSON array
- each array item is one hooker

Example boot config shape:

```toml
[hooker]
default = "None"
plugins = [
  "/absolute/path/dev-a-hookers.json",
  "/absolute/path/dev-b-hookers.json"
]
enabled = []
disabled = []
policies = {}
max_prompt_chain_depth = 128   # 默认值，可不写
```

`max_prompt_chain_depth` bounds cross-turn `send_prompt` chains requested by a `*.Session.lifecycle.state` hooker (see section 16.9). It is an **exclusive** upper bound on `chain_depth`: `N` permits a chain of `N` turns total (the user-initiated turn at depth `0` plus `N - 1` `send_prompt`-triggered turns). Default `128`; the field is optional and defaults when omitted.

Important:

- the example above shows the required JSON shape
- in real usage, you must replace `hook_point` with a value that matches the actual runtime hook point in your app

## 3. Minimal JSON definition

Each plugin hooker item must contain three required fields:

```json
{
  "id": "plugin_read_file_pre_gate",
  "hook_point": "*.Tool.file_read.pre",
  "command": "python3 ./my_hooker/read_file_pre_gate.py"
}
```

Field meaning:

- `id`: unique hooker id in the registry
- `hook_point`: where this hook should run
- `command`: shell command executed by the adaptor

Optional fields you may add:

- `raw_command`: the command as written by the author, before path resolution. `plugins/hookers/install.sh` resolves relative paths / `~` / `VAR=value` tokens in `command` against the hooker directory and, when it rewrites anything, stores the original in `raw_command` and writes the rewritten value back to `command` (`plugins/hookers/install.sh:95-99,118-121`). A shipped plugin uses it to keep the human-readable form: `plugins/hookers/agent_moss/plugin.json:6` has `"raw_command": "python3 bridge.py"` alongside the resolved `command`. You normally do not need to write this field by hand — `install.sh` adds it.
- any other extra JSON field. Extra fields are preserved verbatim in `definition` and passed to the plugin process, so custom settings belong there.

## 4. How to choose the hook point

Current hook point format is:

```text
agent.action.detail.stage
```

The `action` segment selects which hook family the entry belongs to. Today **four** families are supported — `Tool`, `Llm`, `Chat`, `Session`:

- `Tool` — wraps a tool invocation. `stage` must be `pre`, `post`, or `error`.
- `Llm` — wraps a single LLM request/response round-trip. `stage` must be `pre`, `post`, or `error`.
- `Chat` — wraps user input / system prompt assembly. Three sub-points exist (see section 14 below): `*.Chat.command.before`, `*.Chat.message.received`, `*.Chat.system.transform`.
- `Session` — wraps session lifecycle events. Three sub-points exist (see section 15): `*.Session.lifecycle.created`, `*.Session.lifecycle.closed`, `*.Session.lifecycle.state`.

The family is routed on the `action` segment, and the exact hook-point category is resolved from the **`(action, stage)` pair only** (`crates/hook/src/hookers/plugin/builder.rs:21-31`; `crates/hook/src/hookers/hook_point_category.rs:45-62`):

| `action` | accepted `stage` | family |
|---|---|---|
| `Tool` | `pre` / `post` / `error` | tool |
| `Llm` | `pre` / `post` / `error` | llm |
| `Session` | `created` / `closed` / `state` | session |
| `Chat` | `transform` | chat system-transform |
| `Chat` | `received` | chat message |
| `Chat` | `before` | chat command-before |

**The `detail` segment is free-form and is never validated.** The matcher reads the segments as `[agent, action, detail, stage]` and deliberately ignores `detail` (`crates/hook/src/hookers/hook_point_category.rs:34,45-62`). So `*.Chat.message.received` and `*.Chat.whatever.received` select the *same* category; `detail` is a readability/documentation convention (`message` / `system` / `command`, `file_read`, `lifecycle`), not a routing key. Use the conventional `detail` values shown in the examples so your `plugin.json` stays readable, and never branch on `detail` from inside the script — branch on `payload.stage` (section 14.2).

A hook point must have exactly **four** dot-separated segments. Anything else, or an empty `action`/`stage`, or an unlisted `(action, stage)` pair, is a hard config error at registry build time (`crates/hook/src/hookers/hook_point_category.rs:25-43,63-68`) — the daemon refuses to start rather than silently ignoring the hook.

Examples:

- `tool_cli.Tool.file_read.pre`
- `cli-agent.Tool.glob.post`
- `*.Tool.*.pre`
- `*.Llm.complete.pre`
- `*.Chat.message.received`
- `*.Session.lifecycle.created`
- `*.Session.lifecycle.state`

A live example of the `Llm` family ships in this repo: `plugins/hookers/llm_pre_secret_guard/plugin.json:4` registers `*.Llm.complete.pre` (see also its [README](./llm_pre_secret_guard/README.md)).

Wildcard support today:

- only full segment `*`
- allowed example: `*.Tool.*.pre`
- not allowed as wildcard: `tool_*`

## 5. Important matching rule

Your plugin is not matched by `id`.

It is matched by `hook_point`.

That means the `hook_point` must agree with the real runtime values used by the caller.

For example, if the runtime generates:

```text
tool_cli.Tool.file_read.pre
```

then these will match:

- `tool_cli.Tool.file_read.pre`
- `*.Tool.file_read.pre`
- `*.Tool.*.pre`

but this will not match:

- `defaultagent.Tool.file_read.pre`

## 6. Plugin process protocol

The adaptor runs your command with:

```text
sh -c <command>
```

Then it:

- writes one JSON payload to stdin
- waits for the command to exit
- reads one JSON object from stdout

If the command exits non-zero, the hook is treated as failed.

### Subprocess timeout

Every plugin command runs under a **hard timeout**, and the child is killed when it expires (`kill_on_drop`, `crates/hook/src/hookers/plugin/core.rs:297-355`). A plugin script that hangs (deadlock, infinite loop, a network call without its own timeout) fails the hook instead of stalling the agent loop — but a legitimately slow script is still a bug, so keep plugins fast and give any network call its own timeout.

| hook family | cap | where |
|---|---|---|
| `Tool` | 600000 ms (10 min) | `crates/hook/src/hookers/plugin/tool/adaptor.rs:21` |
| `Llm` | 600000 ms (10 min) | `crates/hook/src/hookers/plugin/llm/adaptor.rs:21` |
| `Chat` | 30000 ms (30 s) | `crates/hook/src/hookers/plugin/core.rs:268` |
| `Session` | 30000 ms (30 s) | `crates/hook/src/hookers/plugin/core.rs:268` |

Tool/Llm hooks get the longer cap because their plugins may proxy slow LLM/tool work; chat/session hookers are short observers. The cap covers the **whole** interaction — the stdin write and the stdout wait share one deadline (`crates/hook/src/hookers/plugin/core.rs:312-320`), so a plugin that never reads stdin still cannot hang the host, even when the payload exceeds the pipe buffer.

On timeout the hook fails with a timeout error carrying the cap in milliseconds: for `Tool` this maps to `ToolExecutionError::Timeout { timeout_ms }` (`crates/hook/src/hookers/plugin/tool/adaptor.rs:48-50`), for `Llm` to the dedicated `LlmError::Timeout` variant (`crates/hook/src/hookers/plugin/llm/adaptor.rs:49-51`), and for `Chat`/`Session` to the generic plugin error (`crates/hook/src/hookers/plugin/chat/adaptor.rs:43-45`; `crates/hook/src/hookers/plugin/session/adaptor.rs:59-61`).

A failed hook — spawn failure, non-zero exit, timeout, invalid JSON on stdout, missing required response field, or an unsupported `result` tag — is logged and skipped by the host; it never propagates as a turn failure. Exit non-zero only when you actually mean "this hook failed".

## 7. Pre-hook protocol

### Input payload

Typical pre-hook payload shape:

```json
{
  "stage": "pre",
  "session_id": "s1",
  "workspace": "/home/user/proj",
  "hooker": {
    "id": "plugin_read_file_pre_gate",
    "hook_point": "*.Tool.file_read.pre",
    "command": "python3 script.py",
    "agent_id": "tool_cli"
  },
  "metadata": {
    "trace_id": "…",
    "span_id": "…",
    "parent_span_id": null
  },
  "call": {
    "call_id": "tool-cli-call",
    "tool_name": "file_read",
    "input": {
      "file_path": "/tmp/a.txt"
    }
  },
  "prompt_session": "please read /tmp/a.txt",
  "prompt_history": [
    { "text": "please read /tmp/a.txt", "actions": [] }
  ],
  "action_history": [
    {
      "action_type": "glob",
      "action_detail": { "pattern": "*.txt" },
      "call_id": "prev-call",
      "output": "a.txt",
      "is_error": false
    }
  ],
  "policy": null,
  "definition": {
    "id": "plugin_read_file_pre_gate",
    "hook_point": "*.Tool.file_read.pre",
    "command": "python3 script.py"
  }
}
```

Every plugin payload — all four families, all stages — carries these five common fields (`crates/hook/src/hookers/plugin/core.rs:173-191`):

| field | meaning |
|---|---|
| `stage` | stage discriminator string (see 14.2 for chat values) — `pre` / `post` / `error` for Tool/Llm, `session_created` / `session_closed` / `session_state` for Session |
| `hooker` | `{ id, hook_point, command, agent_id }` — id/hook-point/command as configured plus the ambient agent id (`crates/hook/src/hookers/plugin/core.rs:63-70`) |
| `metadata` | trace/span correlation: `{ trace_id, span_id, parent_span_id }` (`crates/hook/src/hookers/plugin/core.rs:74-80`) |
| `policy` | the effective per-hooker policy `Value` for this hooker id, or `null` when none is configured (`crates/hook/src/hookers/plugin/core.rs:184`) |
| `definition` | your own `plugin.json` entry, verbatim — this is where custom fields land |

Tool `pre` adds these stage fields on top (`crates/hook/src/hookers/plugin/tool/adaptor.rs:292-304`):

| field | type | meaning |
|---|---|---|
| `session_id` | string | session driving the tool call; never `null` — falls back to `call.call_id` when the runtime carries no session id (`crates/hook/src/hookers/plugin/tool/adaptor.rs:142-149`) |
| `workspace` | string \| null | absolute workspace root bound to the agent (`crates/hook/src/hookers/plugin/core.rs:250-255`) |
| `call` | object | the tool call being gated: `{ call_id, tool_name, input }` |
| `prompt_session` | string | the **most recent** user message text, `""` when there is none. This is the "current intent", not the first user message of the session — user answers to `ask_user_question` come back as tool results, not user messages, so this stays the last thing the user actually typed (`crates/hook/src/hookers/plugin/tool/adaptor.rs:166-176`) |
| `prompt_history` | array | interleaved conversation history: one entry per user turn `{ text, actions: [...] }`, where `actions` holds the tool results that followed that turn, each `{ action_type, action_detail, call_id, output, is_error }` (`crates/hook/src/hookers/plugin/tool/adaptor.rs:237-282`). Use this when a later "continue" must not erase the earlier intent. |
| `action_history` | array | flat, chronological list of completed tool calls with their results: `{ action_type, action_detail, call_id, output, is_error }` (`crates/hook/src/hookers/plugin/tool/adaptor.rs:205-231`). `action_type` is the tool name, `action_detail` is that tool's input. |

Both histories are derived from the last 100 conversation messages (`crates/hook/src/hookers/plugin/tool/adaptor.rs:159`).

`session_id` is the id of the session driving the tool call (`null` never happens here; when the runtime carries no session id the call id is used as the fallback identity). `workspace` is the absolute workspace root bound to the agent (`null` when none); the plugin subprocess inherits the host's cwd, which may have drifted (e.g. after a `cd` inside a bash tool), so treat `payload.workspace` — not `process.cwd()` — as the authoritative workspace path. **There is no `cwd` field in the payload.** This is not a cosmetic naming difference: the agent_moss hooker reads `data.get("cwd", "")` and therefore always forwards an empty workspace, silently disabling its indirect-file-access guard. See [`plugins/hookers/agent_moss/docs/KNOWN_ISSUES.md`](agent_moss/docs/KNOWN_ISSUES.md) for the full case and the one-line fix — read `payload.workspace`.

### Allowed output

Allow the call:

```json
{ "result": "allow" }
```

Deny the call:

```json
{ "result": "deny", "reason": "blocked by policy" }
```

Rewrite tool input:

```json
{ "result": "transform", "modified_input": { "file_path": "/safe/path.txt" } }
```

## 8. Post-hook protocol

### Input payload

The post-hook payload carries the same common blocks (`stage` / `hooker` / `metadata` / `policy` / `definition`) plus `session_id`, `workspace`, `call`, and `outcome` (`crates/hook/src/hookers/plugin/tool/adaptor.rs:322-332`).

Success example:

```json
{
  "stage": "post",
  "session_id": "s1",
  "workspace": "/home/user/proj",
  "hooker": { "id": "...", "hook_point": "*.Tool.*.post", "command": "...", "agent_id": "..." },
  "metadata": { "trace_id": "…", "span_id": "…", "parent_span_id": null },
  "call": {
    "call_id": "tool-cli-call",
    "tool_name": "file_read",
    "input": { "file_path": "/tmp/a.txt" }
  },
  "outcome": {
    "type": "success",
    "output": "file content"
  },
  "policy": null,
  "definition": { ... }
}
```

Error output example:

```json
{
  "stage": "post",
  "call": { "call_id": "…", "tool_name": "bash", "input": { ... } },
  "outcome": {
    "type": "error",
    "message": "something went wrong"
  }
}
```

`outcome.type` is either `"success"` (carrying `output`) or `"error"` (carrying `message`) (`crates/hook/src/hookers/plugin/tool/adaptor.rs:363-374`). Note this is a **tool outcome** on the `post` stage — a tool that ran and returned an error message. A tool that *failed to execute* fires the `error` stage instead (section 9). `call` is identical in shape to the pre payload, so post hookers correlate an event with the call that produced it without cross-process state.

### Allowed output

Keep the original result:

```json
{ "result": "accept" }
```

Rewrite successful output text:

```json
{ "result": "transform", "modified_output": "new output" }
```

## 9. Error-hook protocol

### Input payload

The error-hook payload carries the same common blocks (`stage` / `hooker` / `metadata` / `policy` / `definition`) plus `session_id`, `workspace`, `call`, and `error` (`crates/hook/src/hookers/plugin/tool/adaptor.rs:350-360`).

Example:

```json
{
  "stage": "error",
  "session_id": "s1",
  "workspace": "/home/user/proj",
  "hooker": { "id": "...", "hook_point": "*.Tool.*.error", "command": "...", "agent_id": "..." },
  "metadata": { "trace_id": "…", "span_id": "…", "parent_span_id": null },
  "call": {
    "call_id": "tool-cli-call",
    "tool_name": "bash",
    "input": { "command": "cat /etc/shadow" }
  },
  "error": {
    "type": "execution_failed",
    "message": "command failed"
  },
  "policy": null,
  "definition": { ... }
}
```

`error.type` is one of four tags (`crates/hook/src/hookers/plugin/tool/adaptor.rs:376-397`):

| `error.type` | extra fields | meaning |
|---|---|---|
| `not_found` | `tool_name`, `message` | no tool with that name is registered |
| `execution_failed` | `message` | the tool ran and failed |
| `timeout` | `timeout_ms`, `message` | the tool exceeded its own timeout budget |
| `permission_denied` | `message` | policy/permission refused the call |

`message` is always present. Branch on `error.type`, not on the text of `message`. Note that a plugin hooker that *itself* times out surfaces as `ToolExecutionError::Timeout` too (section 6) — i.e. an `error.type: "timeout"` can originate from your own plugin, not only from the tool.

### Allowed output

Keep propagating the error:

```json
{ "result": "propagate" }
```

Recover with replacement output:

```json
{ "result": "recover", "output": "fallback text" }
```

## 10. Example script

Shipped plugins in this repository that you can read as complete, working protocol examples:

- [`tool_post_secret_guard`](./tool_post_secret_guard/README.md) — a `*.Tool.*.post` hooker (`plugins/hookers/tool_post_secret_guard/plugin.json:4`) that reads `payload.call.tool_name` and `payload.outcome` and returns `accept` or `transform`.
- [`llm_pre_secret_guard`](./llm_pre_secret_guard/README.md) — a `*.Llm.complete.pre` hooker (`plugins/hookers/llm_pre_secret_guard/plugin.json:4`) that scans `payload.request` and returns `allow` or `transform`.
- [`cerberus_bash_control`](./cerberus_bash_control/README.md) — a `*.Tool.bash.pre` / `*.Tool.bash.post` pair that rewrites the command and inspects the result.

The smallest possible runnable pre-hook, copied from the pre payload in section 7:

`my_hooker/plugin.json`:

```json
[
  {
    "id": "plugin_read_file_pre_gate",
    "hook_point": "*.Tool.file_read.pre",
    "command": "python3 read_file_pre_gate.py"
  }
]
```

`my_hooker/read_file_pre_gate.py`:

```python
#!/usr/bin/env python3
import json
import sys

payload = json.load(sys.stdin)

if payload.get("stage") != "pre":
    json.dump({"result": "allow"}, sys.stdout)
    sys.exit(0)

file_path = (payload.get("call", {}).get("input") or {}).get("file_path", "")

if file_path == "/etc/passwd":
    json.dump({"result": "deny", "reason": "reading /etc/passwd is not allowed"}, sys.stdout)
else:
    json.dump({"result": "allow"}, sys.stdout)
```

Register `my_hooker/plugin.json` under `[hooker].plugins` (absolute path) and it is live. What matters is the protocol, not the tool: the script switches on `payload.stage`, reads the call from `payload.call`, and writes exactly one JSON object to stdout.

> **`file_read` is a real builtin tool name.** The builtin tools today are `ask_user_question`, `bash`, `count_text_length`, `file_edit`, `file_read`, `file_write`, `glob`, `grep`, `join_subagent`, `lsp`, `print_hello_world`, `send_file`, `skill`, `spawn_subagent`, `todo_write`, `webfetch`, `web_search` (`crates/tool/src/impl/builtin/*/spec.rs`). There is no `read` / `write` / `edit` / `search` tool — a `hook_point` naming one of those never fires.

If you copy this into a real app, make sure the `hook_point` matches that app's real runtime hook point.

## 11. Common mistakes

### Mistake 1: JSON file is not an array

Wrong:

```json
{ "id": "only_one" }
```

Right:

```json
[
  { "id": "only_one", "hook_point": "*.Tool.*.pre", "command": "python3 script.py" }
]
```

### Mistake 2: hook point does not match runtime reality

If runtime uses `tool_cli.Tool.file_read.pre`, then `defaultagent.Tool.file_read.pre` will never trigger.

### Mistake 3: `(action, stage)` pair is unsupported

The legal `stage` depends on the `action` (see section 4). For `Tool` and `Llm`:

- `pre`
- `post`
- `error`

For `Session`: `created`, `closed`, `state`. For `Chat`: `transform`, `received`, `before`. An unlisted pair is a hard config error — the daemon refuses to start.

### Mistake 4: stdout is not valid JSON

Printing logs to stdout will break the protocol.

Write only the result JSON to stdout.

If you need logs, write them to stderr.

### Mistake 5: non-zero exit code

If the command exits with failure, the adaptor treats the hook as failed.

## 12. Practical advice

- start with a pre-hook because it is easiest to reason about
- use `*.Tool.*.pre` if you want broad coverage
- keep plugin scripts small and deterministic
- print protocol JSON only to stdout
- keep extra metadata in the definition JSON if your script needs custom settings

## 13. Checklist before you say "my plugin does not work"

- is the `plugin_hook` feature enabled in the app crate?
- is the plugin file path listed in `HookerRegistryConfig.plugins`?
- is the plugin file a JSON array?
- does each item have `id`, `hook_point`, and `command`?
- is your `(action, stage)` pair one of the supported combinations (section 4)? Remember `detail` is free-form and never validated — a "wrong" `detail` still fires, but a wrong `action`/`stage` is a config error.
- does your `hook_point` really match the runtime hook point, using a **real builtin tool name** (e.g. `file_read`, not `read`)?
- does your script exit with `0`?
- does your script write valid JSON to stdout?
- does your script finish within its family's timeout (30 s for chat/session, 10 min for tool/llm — section 6)?
- for chat/session hooks, does your `payload.stage` match the adaptor's stage string (`command_before` / `chat_message` / `system_transform` / `session_created` / `session_closed` / `session_state`)?
- for session lifecycle hooks, is your result exactly `{"result":"ack"}` (or the alias `acknowledged`)? Note that chat-hook result tags like `allow` / `accept` are **not** accepted here and will be treated as a failure.

## 14. Chat hook protocol

The three chat hooks (`*.Chat.command.before`, `*.Chat.message.received`, `*.Chat.system.transform`) are **mutable** hooks: a plugin may rewrite or deny the input flowing through the agent loop. They share the same subprocess protocol as Tool hooks (`sh -c <command>`, one JSON payload on stdin, one JSON object on stdout, non-zero exit = failure), but the payload shape and the set of legal result tags differ per hook.

> **session_id / workspace**: All three chat hooks carry `payload.session_id` and `payload.workspace`. `session_id` is the **same value** in local and remote mode — the TUI sends its own `session_id` to the daemon via `RuntimeTurnRequest`, and the daemon reuses it verbatim, so a plugin never needs to detect which mode it is running in. Just read `payload.session_id`. (`*.Chat.system.transform` types it as `Option<String>`, so it may serialize as `null`; guard with `payload.session_id || "(unknown)"`.) `workspace` is the absolute workspace root from the agent runtime context (`null` when none is bound); prefer it over the subprocess cwd, which is inherited from the host process and may drift. Tool `pre`/`post`/`error` hooks and the session lifecycle hooks also carry both fields; all `*.Llm.*` hooks currently do **not** — see the table in [`docs/plugins.md`](../../docs/plugins.md) "session_id 与 workspace 获取".

### 14.1 How chat hooks are dispatched

Inside `run_agent_loop`, on the `append_user_message == true` branch, xiaoo fires them in this fixed order:

1. `*.Chat.command.before` — only when the turn originated from a slash command (`CommandContext.is_some()`). A `Deny` short-circuits the whole turn: xiaoo writes a refusal assistant message and returns `Complete` without ever calling the model.
2. `*.Chat.message.received` — fires for every user message before it is persisted, including queued follow-up turns drained by `drain_pending_user_messages`.
3. `*.Chat.system.transform` — fires inside `build_messages` after the prompt builder produces the ordered `system: Vec<String>` parts and before they are joined into the single system message. A `Transform` rewrites both `result.system_parts` and `request.messages[0]` so the LLM actually sees the new system text.

Ordering intent: `command.before` rewrites the command-layer body, which feeds `message.received`, which feeds `system.transform`. Each downstream stage sees the upstream mutation.

Hooks are discovered via `runtime_view.hookers().list_for_hook_point`, filtered by `is_enabled`, sorted by id (predictable order), and invoked sequentially. Each invocation is wrapped in a trace span (`Hook` kind). A single hooker failure (spawn failure, non-zero exit, invalid JSON, missing field, unsupported result tag) is logged to `tracing::warn!` and recorded in the error span, then the loop `continue`s — only `command.before`'s `Deny` actually short-circuits.

### 14.2 `payload.stage` values

`payload.stage` is a discriminator string hardcoded by each adaptor's `build_*_payload`. It is **not** the trailing segment of `hook_point`:

| `hook_point` (in plugin.json) | `payload.stage` |
|---|---|
| `*.Chat.command.before` | `command_before` |
| `*.Chat.message.received` | `chat_message` |
| `*.Chat.system.transform` | `system_transform` |

Scripts should branch on `payload.stage` rather than re-parsing `hook_point`.

### 14.3 `*.Chat.command.before`

Fires after a slash command template is expanded into `body` and before that body is submitted as a user turn.

Input payload shape:

```json
{
  "stage": "command_before",
  "hooker": { "id": "...", "hook_point": "*.Chat.command.before", "command": "...", "agent_id": "..." },
  "metadata": { ... },
  "command": "review",
  "session_id": "s1",
  "workspace": "/home/user/proj",
  "arguments": "src/main.rs",
  "body": "Review this carefully.\n\nsrc/main.rs",
  "policy": null,
  "definition": { ... }
}
```

Legal output:

```json
{ "result": "allow" }
```

```json
{ "result": "transform", "body": "rewritten body" }
```

```json
{ "result": "deny", "reason": "blocked by policy" }
```

`deny` short-circuits the turn: xiaoo writes a refusal assistant message and returns `Complete`. `reason` is optional and defaults to `"denied by plugin"`.

### 14.4 `*.Chat.message.received`

Fires when a user `ChatMessage` is constructed but before it is persisted to message history. Also fires for queued follow-up turns.

Input payload shape (the `message` field is a full `ChatMessage` object — `role`, `blocks`, `timestamp_ms`, etc.):

```json
{
  "stage": "chat_message",
  "hooker": { ... },
  "metadata": { ... },
  "session_id": "s1",
  "workspace": "/home/user/proj",
  "agent": "defaultagent",
  "model": { "provider_id": "...", "model_id": "..." },
  "message_id": null,
  "message": {
    "role": "user",
    "blocks": [ { "type": "text", "text": "hello" } ],
    "timestamp_ms": 0,
    "message_id": null,
    "api_usage_tokens": null,
    "reasoning_content": null,
    "estimated_tokens": null
  },
  "prior_message_count": 0,
  "policy": null,
  "definition": { ... }
}
```

`prior_message_count` is the number of messages already in the conversation history **at the moment this hook fires**. Because the hook fires *before* the current user message is persisted, this count reflects prior messages only — it does NOT include the message under inspection. Use it to detect the "first effective user input" of a session:

| Scenario | `prior_message_count` |
|---|---|
| Brand-new session, first user message | `0` |
| First turn done (user+assistant persisted), second user message | `2` |
| User persisted but assistant interrupted, new message arrives | `1` |

The idiomatic check is `prior_message_count <= 1` (not `=== 0`): the `<= 1` buffer covers retry / interrupted-recovery where only a user message was persisted without an assistant reply, so the current input is still logically the session's first effective turn. This mirrors opencode's `chat.message` first-message detection, but computed in-process from the shared message store (no HTTP callback needed).

Legal output:

```json
{ "result": "accept" }
```

```json
{
  "result": "transform",
  "message": {
    "role": "user",
    "blocks": [ { "type": "text", "text": "redacted" } ],
    "timestamp_ms": 0,
    "message_id": null,
    "api_usage_tokens": null,
    "reasoning_content": null,
    "estimated_tokens": null
  }
}
```

A `Transform` replaces the entire message object. The returned message must be a valid `ChatMessage` (all fields present); otherwise the hooker is treated as failed and skipped.

### 14.5 `*.Chat.system.transform`

Fires inside `build_messages` after the prompt builder produces the ordered `system: Vec<String>` parts and before they are joined into the single system message.

Input payload shape:

```json
{
  "stage": "system_transform",
  "hooker": { ... },
  "metadata": { ... },
  "session_id": "s1",
  "workspace": "/home/user/proj",
  "model": { "provider_id": "...", "model_id": "..." },
  "system": [ "base instruction", "second part" ],
  "policy": null,
  "definition": { ... }
}
```

Legal output:

```json
{ "result": "allow" }
```

```json
{ "result": "transform", "system": [ "new", "parts" ] }
```

`Transform` replaces the entire `system` array. The replacement is written into both `result.system_parts` and `request.messages[0]` (the system message text), so the LLM sees the new system prompt. Multiple hookers chained: each one receives the previous one's `Transform` output as input.

### 14.6 `action: "ask_user"` interaction

The chat hooks support an interactive protocol. Instead of returning a `result`, a plugin may return:

```json
{
  "action": "ask_user",
  "request": {
    "kind": "confirm",
    "prompt": "Allow rewriting the system prompt?"
  },
  "continuation": { "any": "value" }
}
```

`request.kind` selects the interaction widget:

- `confirm` — `{ "kind": "confirm", "prompt": "..." }`
- `text_input` — `{ "kind": "text_input", "prompt": "..." }` (non-secret)
- `choice` — `{ "kind": "choice", "prompt": "...", "options": ["a","b"], "allow_custom_input": true }`

xiaoo presents the widget to the user. After the user answers, xiaoo calls the **same** plugin command again with the original payload augmented by an `interaction` field:

```json
{
  ...original payload...,
  "interaction": {
    "request": { ...the InteractionRequest that was shown... },
    "response": { ...the InteractionResponse the user gave... },
    "continuation": { "any": "value" }
  }
}
```

`interaction.response` is tagged by `kind` (the response tags are named differently from the request kinds):

- `{"kind":"confirmed","allowed":true|false}` — an explicit answer to a `confirm` request: `allowed:true` means the user chose "Yes", `allowed:false` means "No".
- `{"kind":"unanswered"}` — no answer was received (turn cancelled, prompt dismissed, timeout, or no interaction backend available). This is **not** a denial: do not treat it as `allowed:false`; re-issue `action:"ask_user"` if the answer gates a decision.
- `{"kind":"text","value":...}` / `{"kind":"choice","value":...}` — the answer to a `text_input` / `choice` request: `value` is the user's input verbatim (the chosen option text or a custom answer), `null` means no answer was received. On timeout the value may instead be an `[INTERACTION_TIMEOUT]` sentinel string.

The plugin inspects `interaction.response` and either returns another `action: "ask_user"` (loop) or returns a `result` (`final`). When `action` is absent or `"final"`, the `result` is treated as the hook's terminal output. This lets a plugin gate a `Transform`/`Deny` behind explicit user consent.

**Which hooks support `ask_user`**: the round-trip lives in the shared plugin-hooker core, so it is available to the **chat**, **tool**, and **llm** families — not only chat (`crates/hook/src/hookers/plugin/core.rs:193-235`). All three call it through the same `resolve_plugin_output` path: chat (`crates/hook/src/hookers/plugin/chat/adaptor.rs:85`), tool (`crates/hook/src/hookers/plugin/tool/adaptor.rs:85`), llm (`crates/hook/src/hookers/plugin/llm/adaptor.rs:86`). A tool `pre` hooker can therefore ask the user whether to allow a call, and an llm `pre` hooker whether to send a request as-is.

The session lifecycle hooks (section 15) do **not** support `ask_user` — they are event-only, and they call the one-shot subprocess driver directly rather than the interaction loop (`crates/hook/src/hookers/plugin/session/adaptor.rs:136-144`).

## 15. Session lifecycle hook protocol

The `*.Session.lifecycle.*` family — `created`, `closed`, and `state` — consists of **event-style observer** hooks. They are dispatched by `CoreBackedSessionService` in the gateway layer (not inside `agent_loop`): `created` when a session record is first created (first open, the first turn of a brand-new session, or a runtime forked from a checkpoint), `closed` when a session is force-closed (idempotent — an already-closed session does not re-fire), and `state` on root-turn lifecycle state transitions: after a non-error turn termination (`"idle"`) and after a turn terminates with an error (`"failed"`).

### 15.1 Contract

- The only legal result is `{"result":"ack"}` (the alias `acknowledged` is also accepted for ergonomics). Any other tag — including `transform` and the chat-hook tags `allow` / `accept` — is rejected and the hooker is treated as failed, so a plugin that mistakenly reuses a chat-hook result tag gets a loud error rather than silent acceptance.
- There is no `transform` / `deny` path. The events carry no mutable output — plugins are observers.
- The lifecycle state tag is carried in `payload.state`, **not** in the hook point. Two `state` values are emitted today: `"idle"` (after any non-error turn termination) and `"failed"` (after a turn that returned `Err`). The `String` type is intentional so future call sites can emit additional tags without changing this contract or breaking existing plugins.
- The turn's terminal kind is carried in `payload.outcome`. For `state="idle"` it is one of `"complete"` / `"max_turns_reached"` / `"budget_exhausted"` / `"cancelled"` (the four `Ok` variants of `AgentOutcome`), letting plugins distinguish a normal completion from a soft termination. For `state="failed"` it is `"error"` (true failure — the failure path has no `AgentOutcome` variant).
- Dispatch:
  - `created` / `closed`: `fire_session_hooks` awaits each registered hooker sequentially (sorted by id) but ignores the hook output — `actions` are **not** collected for these events. The hooker runs under the same 30s per-subprocess cap.
  - `state="idle"`: `run_turn_inner` calls `fire_session_state_hook_and_collect_actions`, which **awaits** every registered hooker (sorted by id) under a 30s overall deadline and collects their `actions` into `AppTurnResult.hook_actions`. Awaiting is required so action execution can be bundled into the turn's `Done` SSE event before the TUI tears down the stream.
  - `state="failed"`: `run_turn_inner` calls `fire_session_state_hook_background`, which **fire-and-forgets** the hookers via `tokio::spawn` without awaiting. There is no `AppTurnResult` to attach actions to (the turn has failed), so any `actions` requested by the plugin are intentionally discarded — chain-initiated turns only make sense after a turn terminates with `Ok`, so plugins should hook `idle` to chain.
- Plugin errors (spawn failure, non-zero exit, invalid JSON, unsupported result) are logged via `tracing::warn!` and the loop continues to the next hooker. They never affect the turn result or downstream flows.
- The hook is **not** wrapped in a trace span (unlike chat hooks). If you need observability, write to your own log file from inside the script.

### 15.2 Input payload shapes

`*.Session.lifecycle.created`:

```json
{
  "stage": "session_created",
  "session_id": "s1",
  "sender_id": "u1",
  "workspace": "/home/user/proj",
  "hooker": { "id": "...", "hook_point": "*.Session.lifecycle.created", "command": "...", "agent_id": "..." },
  "metadata": { ... },
  "policy": null,
  "definition": { ... }
}
```

`*.Session.lifecycle.closed` has the same shape with `"stage": "session_closed"`.

`*.Session.lifecycle.state` additionally carries the state transition:

```json
{
  "stage": "session_state",
  "state": "idle",
  "outcome": "complete",
  "hooker": { "id": "...", "hook_point": "*.Session.lifecycle.state", "command": "...", "agent_id": "..." },
  "metadata": { ... },
  "session_id": "s1",
  "sender_id": "u1",
  "agent_id": "defaultagent",
  "workspace": "/home/user/proj",
  "policy": null,
  "definition": { ... }
}
```

The hook point sent to the plugin is constructed as `<agent_id>.Session.lifecycle.<stage>`, so `agent_id` is also available inside `hooker.agent_id` and (for `state`) at top level.

`workspace` is the session's workspace root, taken from the session record (these hooks are dispatched with a `NoopRuntimeView`, so the runtime context is not available); it serializes as `null` when the session has no resolvable workspace.

> **session_id**: `payload.session_id` is the current session id. It is identical in local and remote mode (the TUI forwards its `session_id` to the daemon, which reuses it), so a plugin does not need to detect the run mode to obtain the correct id — just read `payload.session_id`. In remote mode the hooker subprocess is spawned by the daemon process; in local mode by the TUI process. The `create_session` / `switch_session` actions in the response only take effect in remote (daemon) mode (see 16.4); in local mode the hooker still runs and `payload.session_id` is still correct, but requested actions are dropped by the TUI.

### 15.3 Legal output

```json
{ "result": "ack" }
```

That is the entire protocol. The adaptor maps it to `SessionHookResult::Acknowledged` and discards anything else.

### 15.4 When each `state` fires

| `state` | When fired | `outcome` value | Actions collected? |
|---|---|---|---|
| `"idle"` | After `handle.run_turn(...)` returns `Ok` — i.e. any non-error turn termination. Covers all four `AgentOutcome` variants: `Complete`, `MaxTurnsReached`, `BudgetExhausted`, `Cancelled`. All four leave the session back in `idle` (ready for the next turn); the variant is distinguishable via `payload.outcome`. | `"complete"` / `"max_turns_reached"` / `"budget_exhausted"` / `"cancelled"` | Yes — awaited; collected into `AppTurnResult.hook_actions`. |
| `"failed"` | After `handle.run_turn(...)` returns an `Err`. The session did not return to `idle` cleanly; the failure carries no `AgentOutcome` variant. | `"error"` | No — fire-and-forget; any `actions` requested by the plugin are discarded. |

Not fired for any path that does not go through `CoreBackedSessionService::run_turn_inner` (e.g. non-root turns, subagent inner steps). The hook fires only at the root-turn boundary in the gateway layer.

> **Action semantics**: only the `idle` state collects and executes plugin-requested actions. Plugins that need to chain a follow-up turn (`send_prompt`) or open a new session (`create_session` / `switch_session`) must do so from the `idle` branch. The `failed` branch is a pure observer — actions it returns are silently dropped because there is no `AppTurnResult` to attach them to.

### 15.5 Minimal example

```js
#!/usr/bin/env node
const fs = require("fs");
const payload = JSON.parse(fs.readFileSync(0, "utf8") || "{}");

if (payload.stage === "session_state") {
  // payload.state is one of: idle / failed
  // payload.outcome:
  //   - idle     -> "complete" / "max_turns_reached" / "budget_exhausted" / "cancelled"
  //   - failed   -> "error"
  fs.appendFileSync("/tmp/xiaoo-session.log",
    `[${new Date().toISOString()}] ${payload.state}: session=${payload.session_id} agent=${payload.agent_id} outcome=${payload.outcome}\n`);
}

process.stdout.write(JSON.stringify({ result: "ack" }));
```

Switching on `payload.state` is intentional — the same hooker script keeps working unchanged when future xiaoo versions emit additional state tags; you simply read `payload.outcome` to tell a normal completion apart from a soft termination, and to distinguish the failed state by its outcome tag.

## 15b. Llm hook protocol

The `*.Llm.*` family wraps a single LLM request/response round-trip: `pre` (before the request is sent), `post` (after the response arrives), and `error` (the call failed). It shares the subprocess protocol and the common payload blocks with the other families, and — like Chat and Tool — it supports the `ask_user` interaction round-trip (section 14.6) and gets the 10-minute subprocess cap (section 6).

**`*.Llm.*` payloads do not carry `session_id` or `workspace`.** Unlike Tool/Chat/Session hooks, the LLM adaptor adds no session-identity fields; its payload is the common skeleton plus `request` / `response` / `error` only (`crates/hook/src/hookers/plugin/llm/adaptor.rs:139-203`). If your plugin needs the session id, it is not in the payload.

### 15b.1 Input payloads

`*.Llm.*.pre` — common blocks plus `request`, the full `LlmRequest` serialization (model, messages, tools, etc.):

```json
{
  "stage": "pre",
  "hooker": { "id": "...", "hook_point": "*.Llm.complete.pre", "command": "...", "agent_id": "..." },
  "metadata": { "trace_id": "…", "span_id": "…", "parent_span_id": null },
  "request": { "messages": [ ... ], "model": { ... } },
  "policy": null,
  "definition": { ... }
}
```

`*.Llm.*.post` adds `response` (`crates/hook/src/hookers/plugin/llm/adaptor.rs:157-179`):

```json
{
  "stage": "post",
  "request": { ... },
  "response": {
    "message": {
      "text": "…",
      "tool_calls": [ { "call_id": "…", "tool_name": "bash", "input": { ... } } ],
      "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0, "cached_tokens": 0 },
      "stop_reason": "end_turn"
    }
  }
}
```

`stop_reason` is one of `end_turn` / `max_tokens` / `tool_use` / `content_filter` (`crates/hook/src/hookers/plugin/llm/adaptor.rs:220-225`).

`*.Llm.*.error` adds `error` instead (`crates/hook/src/hookers/plugin/llm/adaptor.rs:181-203`). `error.type` is one of: `request_failed`, `http_error`, `api_error`, `parse_error`, `rate_limited` (extra `retry_after_ms`), `auth_error`, `model_not_found` (extra `model`), `provider_not_found`, `config_error`, `context_length_exceeded`, `stream_error`, `io_error`, `timeout`, `cancelled`; `message` accompanies every variant (`crates/hook/src/hookers/plugin/llm/adaptor.rs:230-291`).

### 15b.2 Legal output

Pre — allow or rewrite the request:

```json
{ "result": "allow" }
```

```json
{ "result": "transform", "modified_request": { ...a full LlmRequest... } }
```

Post — accept or rewrite the response:

```json
{ "result": "accept" }
```

```json
{ "result": "transform", "modified_response": { "message": { ... } } }
```

Error — propagate or recover:

```json
{ "result": "propagate" }
```

```json
{ "result": "recover", "response": { "message": { ... } } }
```

`modified_request` must deserialize as a complete `LlmRequest`; a partially-specified object is rejected. For `modified_response` / `recover`, the `message` object must contain `tool_calls` (array), `usage` (object), and `stop_reason` (one of the four strings above); `text` and `reasoning_content` are optional strings (`crates/hook/src/hookers/plugin/llm/adaptor.rs:367-496`). A response that omits a required sub-field fails the hook rather than being silently coerced.

See [`llm_pre_secret_guard`](./llm_pre_secret_guard/README.md) for a shipped `*.Llm.complete.pre` example.

## 16. Plugin-requested session actions

Alongside the existing `result` field, a plugin response may carry an `actions` array. Each entry requests a side-effect action that the host executes **after** applying the primary `result`. The flagship use case is a `*.Session.lifecycle.state` hooker that, after observing a turn termination, asks xiaoo to create or switch to a different session (e.g. spawn a debug session when a tool failed, or hand off to a planner session when budget is exhausted).

### 16.1 Which hook can return `actions`

Today only the **session lifecycle state hook** (`*.Session.lifecycle.state`, section 15) parses the `actions` field from the plugin's stdout JSON. The chat and tool adaptors do **not** parse it — appending `actions` to a chat/tool response is a no-op today. This restriction is intentional: it keeps the mutability surface small and matches the post-turn, observer-only nature of the session state hook (the action runs *after* the turn terminates, so it cannot interfere with the turn that produced it).

### 16.2 Response shape

A session state hooker returns the usual `{"result":"ack"}` plus an optional `actions` array:

```json
{
  "result": "ack",
  "actions": [
    { "kind": "create_session", "session_id": "debug-1" },
    { "kind": "switch_session", "session_id": "debug-1" }
  ]
}
```

Field rules:

- `result` is parsed by the existing adaptor logic (unchanged, non-breaking). For the session state hook it must still be `"ack"` (or the alias `"acknowledged"`).
- `actions` is optional. When absent, not an array, or empty, no side effects are requested.
- Each entry is a tagged-union object: `{ "kind": "<variant>", ...fields }`. The `kind` discriminator is matched `snake_case`-style (see `HookAction` in `crates/agent-types/src/hook/action.rs`).
- Invalid entries (unknown `kind`, missing required fields, wrong types, or non-object entries) are **silently skipped** — a single malformed action does not poison the rest of the array. `parse_actions` collects only the entries that deserialize cleanly.

### 16.3 Action kinds

| `kind` | Required fields | Daemon-side effect | TUI-side effect |
|---|---|---|---|
| `create_session` | `session_id: String` | Calls `open_session` (idempotent resume — opens a new session or resumes an existing one with the same id). | Switches focus to that session (transcript restored from daemon state). |
| `switch_session` | `session_id: String` | Calls `open_session` (idempotent resume — ensures the target session exists before the TUI tries to switch to it). | Switches focus to that session (transcript restored from daemon state); no-op if already focused. |
| `send_prompt` | `session_id: String`, `text: String` | Calls `open_session` (idempotent resume — ensures the target session exists); stamps `chain_depth = emitting_turn_depth + 1` (overwriting any plugin-supplied value) so the cross-turn depth cap can be enforced. | Switches focus to that session (no-op if already focused), echoes `text` locally as a user message, and submits the turn via `POST /api/v1/runtimes/input` so the daemon runs the agent loop and the TUI streams the response. **Remote-mode only** — in local mode the action is dropped by the TUI. |

`send_prompt` also accepts a `chain_depth` field, but it is **host-controlled**: plugins must not set it. The daemon overwrites any plugin-supplied value with `emitting_turn_depth + 1` before forwarding, so a plugin cannot forge a low depth to bypass the cross-turn cap (see 16.9). The TUI relays the stamped value back via `RuntimeTurnRequest.chain_depth` so the resulting turn's depth is tracked.

Both kinds collapse to the same daemon call today (`open_session` is idempotent), but the two variants exist so future xiaoo versions can distinguish "create a brand-new session and seed it" from "switch the TUI to an existing session" without breaking the JSON contract.

### 16.4 Execution flow

The dispatcher changed from fire-and-forget to **awaited** for the session state hook specifically so actions can be collected before the turn result is returned.

> **Mode-dependent contract**: `create_session` / `switch_session` / `send_prompt` actions **only take effect in remote (daemon) mode**. In local (non-daemon) mode the hooker is still invoked and its actions are still collected (so the hooker can use `session_state` for logging/auditing), but the actions are dropped at the TUI's `execute_hook_action` step because `remote_base_url()` is None — no session is created, no focus switch happens, no turn is submitted. See step 5 below.

The full flow:

1. `run_turn` returns `Ok(turn_result)`. The session is back in `idle`.
2. `CoreBackedSessionService::fire_session_state_hook_and_collect_actions` invokes every registered `*.Session.lifecycle.state` hooker **sequentially** (sorted by id), awaiting each one. The `actions` arrays returned by all hookers are concatenated, then each `send_prompt` is **stamped** with `chain_depth = emitting_turn_depth + 1` (overwriting any plugin-supplied value) and **dropped** if that value **reaches** `max_prompt_chain_depth` (`next_depth >= max`, an exclusive upper bound — `N` permits N turns total in a chain; see 16.9). The surviving actions are concatenated into `AppTurnResult.hook_actions`. The whole collection is bounded by an overall deadline of 30s (`SESSION_STATE_HOOK_OVERALL_DEADLINE`, equal to one hooker's per-subprocess cap): a single legitimately slow hooker is unaffected, but the sum across N hookers is capped at 30s instead of N × 30s. On timeout the spawned task is aborted (the in-flight subprocess is reaped via `kill_on_drop`) and **no actions are returned** — best-effort. This step is identical in local and remote mode.
3. **Remote mode only**: The HTTP router (`apps/serverside/src/httpserver/router.rs`) calls `DaemonHookActionSink::execute_on_daemon(hook_actions)`. For each action the daemon runs the daemon-side effect (currently `open_session` for all three kinds — idempotent resume, ensuring the target session exists). For `send_prompt`, the `text` and daemon-stamped `chain_depth` ride along on the forwarded action; the daemon does **not** run the turn itself here (the SSE stream for the new turn can only be obtained by the TUI POSTing `/runtimes/input`). Actions that fail daemon-side execution are **filtered out** and logged via `tracing::warn!`; they are not forwarded to the TUI. (In local mode this step does not exist — there is no daemon, so no `open_session` runs.)
4. **Remote mode only**: The surviving actions are bundled into the SSE `Done` event's `actions` field and sent to the TUI. They are emitted **before** the `Done` event itself — the TUI's receive loop exits as soon as it sees `Done` (it sets `stream_rx = None`), so any update sent after `Done` would never be drained. (In local mode actions travel via `SessionTurnUpdate::HookActions` on the in-process channel, not via SSE.)
5. The TUI buffers the actions in `pending_hook_actions`, then the App event loop drains them via `take_pending_hook_actions()` and dispatches each via `execute_hook_action`:
   - `create_session` / `switch_session`: calls `switch_to_remote_session(session_id)`. (`switch_session` skips if already focused on the target, avoiding a redundant transcript reload + system message.)
     - **Remote mode**: switching focus re-calls `open_session` (idempotent) to fetch the target session's `SessionRecord` and reconstructs the local transcript from `loop_state.messages` so the user sees the prior turns instead of a blank transcript.
     - **Local mode**: `switch_to_remote_session` checks `remote_base_url()`, finds `None`, logs `tracing::warn!("switch_to_remote_session called without remote backend; hook action dropped (local mode does not execute daemon-side actions)")`, and returns without switching. The action is a no-op.
   - `send_prompt`: if `remote_base_url()` is None (local mode), drops the action with `tracing::warn!("send_prompt hook action dropped (local mode does not execute daemon-side actions)")`. Otherwise: switches focus to `session_id` (skipped if already focused), then — if a turn is already running (`is_loading = true`) — enqueues the `text` (carrying `chain_depth`) into `pending_turns` for `start_next_queued_turn` to drain once idle; else calls `start_turn_for_hook_prompt(text, chain_depth)`, which echoes `text` as a user message (`Message::user(prompt)`) and POSTs `/api/v1/runtimes/input` with `chain_depth` set on the request. The daemon runs the agent loop; the TUI streams the SSE response. When that turn ends, step 2 repeats with the new `emitting_turn_depth = chain_depth`, re-enforcing the cap (16.9).

> **Trade-off vs the previous fire-and-forget design**: the user sees the turn result *after* the session state hookers finish (or time out). This is acceptable because session lifecycle hooks only fire after turn termination anyway — there is no LLM streaming to interrupt — and most setups register zero or fast hookers. If your hooker script is slow (e.g. calls an external audit service), it will delay the `Done` event proportionally; keep `actions`-emitting hookers fast.

### 16.5 Best-effort semantics

Actions are best-effort at every layer:

- **Plugin layer**: `parse_actions` skips invalid entries (see 16.2).
- **Daemon layer** (remote mode only): `DaemonHookActionSink::execute_on_daemon` catches per-action failures (e.g. `open_session` returns `Err`) and filters them out — the failed action is logged via `tracing::warn!` with `action`, `session_id`, and `error` fields, and is **not** forwarded to the TUI.
- **Collection layer**: `fire_session_state_hook_and_collect_actions` enforces a 30s overall deadline (`SESSION_STATE_HOOK_OVERALL_DEADLINE`). If the sequential hooker chain does not finish in time, the task is aborted and **all** actions (including ones from hookers that already finished) are dropped — `Done` is delivered with an empty `actions` array. Keep `actions`-emitting hookers fast.
- **TUI layer**: `switch_to_remote_session` is best-effort. In remote mode, if the daemon-side `open_session` somehow failed and the action was still forwarded (it shouldn't be, but defensive coding applies), the TUI logs a `tracing::warn!` and continues without crashing. In local mode, the action is dropped at the `remote_base_url() == None` check (also via `tracing::warn!`) — this is the intended contract, not a failure.

No action failure is propagated back to the caller of the hook. A plugin that requests an unreachable `session_id` simply sees no session switch happen — the user is not shown an error.

### 16.6 Depth limiting

Two distinct depth limits apply to actions:

- **Per-batch cap (`MAX_ACTION_DEPTH = 3`)**: prevents a single hook response from chaining too many actions in one batch. `DaemonHookActionSink::execute_on_daemon` truncates any batch longer than the limit: only the first `MAX_ACTION_DEPTH` entries are processed, the trailing actions are dropped and logged via `tracing::warn!` with `requested` and `max` fields. The canonical `create_session + switch_session + send_prompt` triple exactly fills this budget; do not batch more than three actions in a single response.
- **Cross-turn cap (`max_prompt_chain_depth`, default 128)**: prevents a `send_prompt`-triggered turn from firing another `send_prompt` indefinitely across turns. See 16.9 for the round-trip and enforcement details.

Do not rely on chaining actions across multiple turns beyond the cross-turn cap, or batching more than three actions in a single hook response.

### 16.7 Minimal example

A `*.Session.lifecycle.state` hooker that opens a debug session whenever a turn ends with the `budget_exhausted` outcome, switches the user's focus to it, and sends a follow-up prompt so the agent continues investigating on the new session:

```js
#!/usr/bin/env node
const fs = require("fs");
const payload = JSON.parse(fs.readFileSync(0, "utf8") || "{}");

let result = { result: "ack" };

if (payload.stage === "session_state" && payload.state === "idle") {
  fs.appendFileSync("/tmp/xiaoo-actions.log",
    `[${new Date().toISOString()}] idle: session=${payload.session_id} outcome=${payload.outcome}\n`);

  if (payload.outcome === "budget_exhausted") {
    // Spawn a debug session on the daemon, switch the TUI to it, and send
    // a follow-up prompt. The daemon calls open_session (idempotent resume)
    // for all three actions first; the TUI then switches focus and submits
    // the prompt so the agent auto-continues on the new session.
    //
    // Order matters: send_prompt MUST come last (see 16.8) — a following
    // switch_session would reset TUI state and interrupt the just-started
    // turn's stream. The create+switch+send_prompt triple exactly fills
    // the max action depth of 3.
    const debugId = `debug-${payload.session_id}`;
    result = {
      result: "ack",
      actions: [
        { kind: "create_session", session_id: debugId },
        { kind: "switch_session", session_id: debugId },
        { kind: "send_prompt", session_id: debugId, text: "Continue investigating the failure you were working on." }
      ]
    };
  }
}

process.stdout.write(JSON.stringify(result));
```

Notes on this example:

- The three actions are dispatched **in order**: the daemon opens `debug-<src>` first (or no-ops if it already exists), then ensures the same id exists again (idempotent), then ensures it exists once more for the `send_prompt` and stamps its `chain_depth`. The TUI switches focus once (create), skips the redundant switch (already focused), then echoes the prompt and starts the turn.
- If `open_session` fails on the daemon (e.g. the daemon's session store is unhealthy), all three actions are filtered out and the TUI never sees them; the user stays on the original session.
- Combining `actions` with `ask_user` is **not** supported — the session state hook does not implement the `ask_user` interaction protocol (section 14.6). If you need user confirmation before opening a debug session, return `actions` unconditionally and let the user dismiss the new session manually, or move that logic into a `*.Chat.command.before` hooker instead.
- The `text` of `send_prompt` will pass through the daemon's `*.Chat.message.received` hook on the resulting turn (see 16.8 re-entrancy). If the same hooker handles `chat_message`, do not re-emit `send_prompt` from there.

### 16.8 Cross-turn re-entrancy and ordering constraints

`send_prompt` triggers a normal turn on the target session. That turn's lifecycle (`*.Chat.message.received` at start, `*.Session.lifecycle.state` at end) fires again, so plugin authors must be aware of two re-entrancy surfaces:

- **`chat_message` sees the plugin's own `text`**: the daemon's `agent_loop` fires `*.Chat.message.received` for the `send_prompt` text exactly as for a user-typed message. If the same plugin handles both `chat_message` and `session_state`, its `chat_message` branch will observe the `text` it itself emitted. Do not re-emit `send_prompt` (or otherwise mutate) from `chat_message` based on this self-generated input — it would form a second amplification path on top of the `session_state` chain.
- **`prior_message_count` semantics**: for a freshly created session, the `send_prompt` text is the first message, so `chat_message` sees `prior_message_count = 0` and any "first message" logic (context injection, memory load, etc.) fires for it. This may or may not be desired; design accordingly.
- **Echo vs transform**: if a `chat_message` hooker transforms the `send_prompt` text, the TUI echoes the original text locally while the daemon runs the turn with the transformed text. On the next transcript restore the TUI fetches the transformed version. This is inherited from remote + `chat_message` transform behavior, not new to `send_prompt`.

Ordering within a single `actions` batch:

- The TUI processes actions sequentially in the order they appear. `send_prompt` **must be last** and at most one per batch: a `switch_session` (or `create_session`) after a `send_prompt` would reset TUI state (transcript reload, `is_loading` cleared) and orphan the just-started turn's SSE stream. The `max action depth = 3` plus the canonical `create + switch + send_prompt` triple naturally enforces this.
- If the TUI is already focused on the `send_prompt`'s target session, the switch step is skipped (no transcript reload, no "Switched to session" system message); the prompt is echoed and the turn submitted directly.
- If a turn is already running when `send_prompt` is processed (`is_loading = true`, e.g. the previous turn's reveal buffer hasn't fully drained), the prompt is enqueued in `pending_turns` and drained by `start_next_queued_turn` once idle — identical to a user typing while a turn runs.

### 16.9 Cross-turn `send_prompt` depth cap

A `send_prompt`-triggered turn ends by firing `*.Session.lifecycle.state` again, which may emit another `send_prompt`, forming a cross-turn chain. xiaoo does **not** impose semantic termination conditions on chains (that is the plugin's responsibility), but provides a host-side hard depth cap so a runaway plugin cannot loop forever:

- **Config**: `[hooker].max_prompt_chain_depth`, default `128`.
- **Semantics**: `N` is an **exclusive upper bound** on `chain_depth`. A chain may run **N turns total** — the user-initiated turn at depth `0` plus `N - 1` `send_prompt`-triggered turns at depths `1..=N-1`. A `send_prompt` that would start the turn at depth `N` is dropped.
- **Enforcement point**: the daemon's `fire_session_state_hook_and_collect_actions` stamps each collected `send_prompt` with `chain_depth = emitting_turn_depth + 1` (overwriting any plugin-supplied value). If the stamped value **reaches** `max_prompt_chain_depth` (`next_depth >= max_prompt_chain_depth`), the action is **dropped** (not forwarded to the TUI) and `tracing::warn!` records it; the chain terminates there.
- **Round-trip**: daemon stamps → TUI relays the stamped `chain_depth` back via `RuntimeTurnRequest.chain_depth` → daemon records the new turn's depth → on that turn's end, stamps `+1` and re-checks. The cap is re-evaluated every turn, so there is no escape via the enqueue path or session switching.
- **Reset**: a normal user-typed turn carries `chain_depth = 0`, which immediately resets the chain. Human input always breaks an in-progress chain.
- **No forgery**: plugins cannot set `chain_depth` to bypass the cap — the daemon overwrites it unconditionally. The TUI is host code (not a plugin), so trusting it to relay the value is within the threat model.

```toml
[hooker]
max_prompt_chain_depth = 128   # default; lower it for debugging
```

The cap is exclusive on `chain_depth`: the daemon allows `next = N + 1` while `next < max`, and drops at `next >= max`. So with the default of 128, a chain may run **128 turns total** (1 user-initiated + 127 `send_prompt`-triggered) before being cut off. Setting `N = 3` yields exactly 3 sessions: the user turn (depth 0) plus 2 `send_prompt`-triggered turns (depths 1 and 2); the `send_prompt` that would have started depth 3 is dropped.

### 16.10 Checklist before you say "my actions don't fire"

- is your hooker registered under `*.Session.lifecycle.state` (the only hook point whose adaptor parses `actions` today)?
- does your response JSON include both `"result": "ack"` **and** a top-level `"actions"` array? (`actions` nested inside `result` is silently ignored.)
- is every entry an object with a `kind` field equal to `create_session`, `switch_session`, or `send_prompt` (snake_case)? Unknown kinds are skipped.
- does every entry carry its required `session_id` field as a string? `send_prompt` additionally requires `text`.
- did you **not** set `chain_depth` on `send_prompt`? It is host-controlled; the daemon overwrites it anyway.
- is `send_prompt` the **last** entry in your `actions` array, and at most one per batch? A following `switch_session`/`create_session` would interrupt the just-started turn.
- in remote/daemon mode, are you checking the daemon logs for `daemon-side create_session action failed; not forwarding to TUI` warnings? A failing `open_session` filters the action out before it reaches the TUI.
- if your `send_prompt` chain seems to stop early, check the daemon logs for `send_prompt hook action dropped: chain depth reaches cap` — you may have hit `max_prompt_chain_depth` (default 128, exclusive upper bound on `chain_depth`; `N` permits N turns total in a chain). Raise the cap or fix the plugin's termination logic.
- in local (non-daemon) mode, are you aware that `create_session` / `switch_session` / `send_prompt` are **no-ops**? The hooker still runs (so your logging side-effects still happen), but the actions are dropped at the TUI's `switch_to_remote_session` because `remote_base_url()` is None — you will see a `tracing::warn!` reading `switch_to_remote_session called without remote backend; hook action dropped (local mode does not execute daemon-side actions)` (for create/switch) or `send_prompt hook action dropped (local mode does not execute daemon-side actions)` (for send_prompt). To exercise these actions, run the daemon and connect the TUI in remote mode (see `docs/remote_tui.md`).
