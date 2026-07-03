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
```

Important:

- the example above shows the required JSON shape
- in real usage, you must replace `hook_point` with a value that matches the actual runtime hook point in your app

## 3. Minimal JSON definition

Each plugin hooker item must contain three required fields:

```json
{
  "id": "plugin_read_file_pre_gate",
  "hook_point": "*.Tool.builtin_read_file.pre",
  "command": "python3 crates/hooker/tests/plugin/scripts/read_file_pre_gate.py"
}
```

Field meaning:

- `id`: unique hooker id in the registry
- `hook_point`: where this hook should run
- `command`: shell command executed by the adaptor

You may add extra JSON fields. They are preserved in `definition` and passed to the plugin process.

## 4. How to choose the hook point

Current hook point format is:

```text
agent.action.detail.stage
```

The `action` segment selects which hook family the entry belongs to. Today three families are supported:

- `Tool` — wraps a tool invocation. `stage` must be `pre`, `post`, or `error`.
- `Chat` — wraps user input / system prompt assembly. Three sub-points exist (see sections 14 below): `*.Chat.command.before`, `*.Chat.message.received`, `*.Chat.system.transform`. These are matched as full hook points; the trailing segment is the hook name, not a free-form `stage`.
- `Session` — wraps session lifecycle events. Currently only `*.Session.lifecycle.state` is emitted (see section 15).

Examples:

- `tool_cli.Tool.file_read.pre`
- `cli-agent.Tool.glob.post`
- `*.Tool.*.pre`
- `*.Chat.message.received`
- `*.Session.lifecycle.state`

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

## 7. Pre-hook protocol

### Input payload

Typical pre-hook payload shape:

```json
{
  "stage": "pre",
  "hooker": {
    "id": "plugin_read_file_pre_gate",
    "hook_point": "*.Tool.builtin_read_file.pre",
    "command": "python3 script.py",
    "agent_id": "tool_cli"
  },
  "call": {
    "call_id": "tool-cli-call",
    "tool_name": "file_read",
    "input": {
      "file_path": "/tmp/a.txt"
    }
  },
  "policy": null,
  "definition": {
    "id": "plugin_read_file_pre_gate",
    "hook_point": "*.Tool.builtin_read_file.pre",
    "command": "python3 script.py"
  }
}
```

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

The post-hook payload is like pre-hook, but also contains `outcome`.

Success example:

```json
{
  "stage": "post",
  "outcome": {
    "type": "success",
    "output": "file content"
  }
}
```

Error output example:

```json
{
  "stage": "post",
  "outcome": {
    "type": "error",
    "message": "something went wrong"
  }
}
```

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

The error-hook payload is like pre-hook, but also contains `error`.

Example:

```json
{
  "stage": "error",
  "error": {
    "type": "execution_failed",
    "message": "command failed"
  }
}
```

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

This repository already has a small pre-hook example:

- definition file: `crates/hooker/tests/plugin/tool_pre_read_file_example.json`
- script file: `crates/hooker/tests/plugin/scripts/read_file_pre_gate.py`

What it does:

- checks that `stage == "pre"`
- reads `call.input.file_path`
- denies the call if the path is `/etc/passwd`
- otherwise allows it

Treat this repository example as a protocol example first. If you copy it into a real app, make sure the `hook_point` matches that app's real runtime hook point.

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

### Mistake 3: stage is unsupported

Use only:

- `pre`
- `post`
- `error`

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
- does your `hook_point` really match the runtime hook point?
- does your script exit with `0`?
- does your script write valid JSON to stdout?
- for chat/session hooks, does your `payload.stage` match the adaptor's stage string (`command_before` / `chat_message` / `system_transform` / `session_state`)?
- for session state hooks, is your result exactly `{"result":"ack"}` (or the alias `acknowledged`)? Note that chat-hook result tags like `allow` / `accept` are **not** accepted here and will be treated as a failure.

## 14. Chat hook protocol

The three chat hooks (`*.Chat.command.before`, `*.Chat.message.received`, `*.Chat.system.transform`) are **mutable** hooks: a plugin may rewrite or deny the input flowing through the agent loop. They share the same subprocess protocol as Tool hooks (`sh -c <command>`, one JSON payload on stdin, one JSON object on stdout, non-zero exit = failure), but the payload shape and the set of legal result tags differ per hook.

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
  "policy": null,
  "definition": { ... }
}
```

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

The three chat hooks support an interactive protocol. Instead of returning a `result`, a plugin may return:

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

The plugin inspects `interaction.response` and either returns another `action: "ask_user"` (loop) or returns a `result` (`final`). When `action` is absent or `"final"`, the `result` is treated as the hook's terminal output. This lets a plugin gate a `Transform`/`Deny` behind explicit user consent.

The session state hook (section 15) does **not** support `ask_user` — it is event-only.

## 15. Session lifecycle state hook protocol

`*.Session.lifecycle.state` is an **event-style observer** hook. It is dispatched by `CoreBackedSessionService::run_turn` in the gateway layer (not inside `agent_loop`) after a non-error root turn termination, when the session returns to the `idle` state.

### 15.1 Contract

- The only legal result is `{"result":"ack"}` (the alias `acknowledged` is also accepted for ergonomics). Any other tag — including `transform` and the chat-hook tags `allow` / `accept` — is rejected and the hooker is treated as failed, so a plugin that mistakenly reuses a chat-hook result tag gets a loud error rather than silent acceptance.
- There is no `transform` / `deny` path. The event carries no mutable output — plugins are observers.
- The lifecycle state tag is carried in `payload.state`, **not** in the hook point. Today only `"idle"` is emitted (after any non-error turn termination). The `String` type is intentional so future call sites can emit `"running"` / `"failed"` / ... without changing this contract or breaking existing plugins.
- The turn's terminal kind is carried in `payload.outcome` (`"complete"` / `"max_turns_reached"` / `"budget_exhausted"` / `"cancelled"`). It is populated for every fired event so plugins can distinguish a normal completion from a soft termination while still seeing the same `state="idle"`.
- Dispatch is **fire-and-forget**: `run_turn` clones `session_id` / `sender_id` / `agent_id`, calls `handle.run_turn(...)`, and only if that returns `Ok` does it `tokio::spawn` a background task that invokes all registered state hookers. `run_turn` returns its original `turn_result` to the caller immediately — the spawn handle is not awaited.
- Plugin errors (spawn failure, non-zero exit, invalid JSON, unsupported result) are logged via `tracing::warn!` and the loop continues to the next hooker. They never affect the turn result or downstream flows.
- The hook is **not** wrapped in a trace span (unlike chat hooks). If you need observability, write to your own log file from inside the script.

### 15.2 Input payload shape

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
  "policy": null,
  "definition": { ... }
}
```

The hook point sent to the plugin is constructed as `<agent_id>.Session.lifecycle.state`, so `agent_id` is also available inside `hooker.agent_id` and at top level.

### 15.3 Legal output

```json
{ "result": "ack" }
```

That is the entire protocol. The adaptor maps it to `SessionHookResult::Acknowledged` and discards anything else.

### 15.4 When `idle` is (and is not) fired

- Fired: after `handle.run_turn(...)` returns `Ok` — i.e. any non-error turn termination. This covers all four `AgentOutcome` variants: `Complete`, `MaxTurnsReached`, `BudgetExhausted`, and `Cancelled`. All four leave the session back in `idle` (ready for the next turn), so `state="idle"` is correct for each; the variant is distinguishable via `payload.outcome`.
- Not fired: if `run_turn` returns an `Err`. The failure path currently emits no state event.
- Not fired: for non-root turns or any code path that does not go through `CoreBackedSessionService::run_turn`.

If your plugin needs to react to failures, hook a different signal today; this contract may grow new `state` values (e.g. `"failed"`) in the future, but only `idle` is currently emitted.

### 15.5 Minimal example

```js
#!/usr/bin/env node
const fs = require("fs");
const payload = JSON.parse(fs.readFileSync(0, "utf8") || "{}");

if (payload.stage === "session_state" && payload.state === "idle") {
  // payload.outcome is one of: complete / max_turns_reached / budget_exhausted / cancelled
  fs.appendFileSync("/tmp/xiaoo-session.log",
    `[${new Date().toISOString()}] idle: session=${payload.session_id} agent=${payload.agent_id} outcome=${payload.outcome}\n`);
}

process.stdout.write(JSON.stringify({ result: "ack" }));
```

The `state` branch is intentional — the same hooker script will keep working unchanged when future xiaoo versions emit `running` / `failed` / etc.; you simply add another `case`. Reading `payload.outcome` lets audit-style plugins tell a normal completion apart from a soft termination without switching on `state`.
