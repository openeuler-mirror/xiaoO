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

> **session_id**: All three chat hooks carry `payload.session_id`. It is the **same value** in local and remote mode — the TUI sends its own `session_id` to the daemon via `RuntimeTurnRequest`, and the daemon reuses it verbatim, so a plugin never needs to detect which mode it is running in. Just read `payload.session_id`. (`*.Chat.system.transform` types it as `Option<String>`, so it may serialize as `null`; guard with `payload.session_id || "(unknown)"`.) Tool `pre` hooks and the session lifecycle state hook also carry `session_id`; Tool `post`/`error` and all `*.Llm.*` hooks currently do **not** — see the table in [`docs/plugins.md`](../../docs/plugins.md) "session_id 获取".

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

> **session_id**: `payload.session_id` is the current session id. It is identical in local and remote mode (the TUI forwards its `session_id` to the daemon, which reuses it), so a plugin does not need to detect the run mode to obtain the correct id — just read `payload.session_id`. In remote mode the hooker subprocess is spawned by the daemon process; in local mode by the TUI process. The `create_session` / `switch_session` actions in the response only take effect in remote (daemon) mode (see 16.4); in local mode the hooker still runs and `payload.session_id` is still correct, but requested actions are dropped by the TUI.

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
| `switch_session` | `session_id: String` | Calls `open_session` (idempotent resume — ensures the target session exists before the TUI tries to switch to it). | Switches focus to that session (transcript restored from daemon state). |

Both kinds collapse to the same daemon call today (`open_session` is idempotent), but the two variants exist so future xiaoo versions can distinguish "create a brand-new session and seed it" from "switch the TUI to an existing session" without breaking the JSON contract.

### 16.4 Execution flow

The dispatcher changed from fire-and-forget to **awaited** for the session state hook specifically so actions can be collected before the turn result is returned.

> **Mode-dependent contract**: `create_session` / `switch_session` actions **only take effect in remote (daemon) mode**. In local (non-daemon) mode the hooker is still invoked and its actions are still collected (so the hooker can use `session_state` for logging/auditing), but the actions are dropped at the TUI's `switch_to_remote_session` step because `remote_base_url()` is None — no session is created, no focus switch happens. See step 5 below.

The full flow:

1. `run_turn` returns `Ok(turn_result)`. The session is back in `idle`.
2. `CoreBackedSessionService::fire_session_state_hook_and_collect_actions` invokes every registered `*.Session.lifecycle.state` hooker **sequentially** (sorted by id), awaiting each one. The `actions` arrays returned by all hookers are concatenated into `AppTurnResult.hook_actions`. The whole collection is bounded by an overall deadline of 30s (`SESSION_STATE_HOOK_OVERALL_DEADLINE`, equal to one hooker's per-subprocess cap): a single legitimately slow hooker is unaffected, but the sum across N hookers is capped at 30s instead of N × 30s. On timeout the spawned task is aborted (the in-flight subprocess is reaped via `kill_on_drop`) and **no actions are returned** — best-effort. This step is identical in local and remote mode.
3. **Remote mode only**: The HTTP router (`apps/serverside/src/httpserver/router.rs`) calls `DaemonHookActionSink::execute_on_daemon(hook_actions)`. For each action the daemon runs the daemon-side effect (currently `open_session` for both kinds). Actions that fail daemon-side execution are **filtered out** and logged via `tracing::warn!`; they are not forwarded to the TUI. (In local mode this step does not exist — there is no daemon, so no `open_session` runs.)
4. **Remote mode only**: The surviving actions are bundled into the SSE `Done` event's `actions` field and sent to the TUI. They are emitted **before** the `Done` event itself — the TUI's receive loop exits as soon as it sees `Done` (it sets `stream_rx = None`), so any update sent after `Done` would never be drained. (In local mode actions travel via `SessionTurnUpdate::HookActions` on the in-process channel, not via SSE.)
5. The TUI buffers the actions in `pending_hook_actions`, then the App event loop drains them via `take_pending_hook_actions()` and calls `switch_to_remote_session(session_id)` for each.
   - **Remote mode**: switching focus re-calls `open_session` (idempotent) to fetch the target session's `SessionRecord` and reconstructs the local transcript from `loop_state.messages` so the user sees the prior turns instead of a blank transcript.
   - **Local mode**: `switch_to_remote_session` checks `remote_base_url()`, finds `None`, logs `tracing::warn!("switch_to_remote_session called without remote backend; hook action dropped (local mode does not execute daemon-side actions)")`, and returns without switching. The action is a no-op.

> **Trade-off vs the previous fire-and-forget design**: the user sees the turn result *after* the session state hookers finish (or time out). This is acceptable because session lifecycle hooks only fire after turn termination anyway — there is no LLM streaming to interrupt — and most setups register zero or fast hookers. If your hooker script is slow (e.g. calls an external audit service), it will delay the `Done` event proportionally; keep `actions`-emitting hookers fast.

### 16.5 Best-effort semantics

Actions are best-effort at every layer:

- **Plugin layer**: `parse_actions` skips invalid entries (see 16.2).
- **Daemon layer** (remote mode only): `DaemonHookActionSink::execute_on_daemon` catches per-action failures (e.g. `open_session` returns `Err`) and filters them out — the failed action is logged via `tracing::warn!` with `action`, `session_id`, and `error` fields, and is **not** forwarded to the TUI.
- **Collection layer**: `fire_session_state_hook_and_collect_actions` enforces a 30s overall deadline (`SESSION_STATE_HOOK_OVERALL_DEADLINE`). If the sequential hooker chain does not finish in time, the task is aborted and **all** actions (including ones from hookers that already finished) are dropped — `Done` is delivered with an empty `actions` array. Keep `actions`-emitting hookers fast.
- **TUI layer**: `switch_to_remote_session` is best-effort. In remote mode, if the daemon-side `open_session` somehow failed and the action was still forwarded (it shouldn't be, but defensive coding applies), the TUI logs a `tracing::warn!` and continues without crashing. In local mode, the action is dropped at the `remote_base_url() == None` check (also via `tracing::warn!`) — this is the intended contract, not a failure.

No action failure is propagated back to the caller of the hook. A plugin that requests an unreachable `session_id` simply sees no session switch happen — the user is not shown an error.

### 16.6 Depth limiting

The host enforces a max action depth of 3 (see `agent_contracts::MAX_ACTION_DEPTH`) to prevent recursive `hook → action → hook` loops (a `create_session` action that opens a session whose first turn immediately fires another state hook that requests another `create_session`, ...). `DaemonHookActionSink::execute_on_daemon` truncates any batch longer than the limit: only the first `MAX_ACTION_DEPTH` entries are processed, the trailing actions are dropped and logged via `tracing::warn!` with `requested` and `max` fields. Do not rely on chaining actions across multiple turns or batching more than three actions in a single hook response.

### 16.7 Minimal example

A `*.Session.lifecycle.state` hooker that opens a debug session whenever a turn ends with the `budget_exhausted` outcome, then switches the user's focus to it:

```js
#!/usr/bin/env node
const fs = require("fs");
const payload = JSON.parse(fs.readFileSync(0, "utf8") || "{}");

let result = { result: "ack" };

if (payload.stage === "session_state" && payload.state === "idle") {
  fs.appendFileSync("/tmp/xiaoo-actions.log",
    `[${new Date().toISOString()}] idle: session=${payload.session_id} outcome=${payload.outcome}\n`);

  if (payload.outcome === "budget_exhausted") {
    // Spawn a debug session on the daemon and switch the TUI to it.
    // The daemon calls open_session (idempotent resume) first, so a
    // second budget_exhausted on the same source session won't create
    // a duplicate debug session — it'll just re-focus the existing one.
    const debugId = `debug-${payload.session_id}`;
    result = {
      result: "ack",
      actions: [
        { kind: "create_session", session_id: debugId },
        { kind: "switch_session", session_id: debugId }
      ]
    };
  }
}

process.stdout.write(JSON.stringify(result));
```

Notes on this example:

- The two actions are dispatched **in order**: the daemon opens `debug-<src>` first (or no-ops if it already exists), then ensures the same id exists again (idempotent), then forwards both actions to the TUI. The TUI switches focus once per forwarded action — both target the same `session_id`, so the net effect is one switch.
- If `open_session` fails on the daemon (e.g. the daemon's session store is unhealthy), both actions are filtered out and the TUI never sees them; the user stays on the original session.
- Combining `actions` with `ask_user` is **not** supported — the session state hook does not implement the `ask_user` interaction protocol (section 14.6). If you need user confirmation before opening a debug session, return `actions` unconditionally and let the user dismiss the new session manually, or move that logic into a `*.Chat.command.before` hooker instead.

### 16.8 Checklist before you say "my actions don't fire"

- is your hooker registered under `*.Session.lifecycle.state` (the only hook point whose adaptor parses `actions` today)?
- does your response JSON include both `"result": "ack"` **and** a top-level `"actions"` array? (`actions` nested inside `result` is silently ignored.)
- is every entry an object with a `kind` field equal to `create_session` or `switch_session` (snake_case)? Unknown kinds are skipped.
- does every entry carry its required `session_id` field as a string?
- in remote/daemon mode, are you checking the daemon logs for `daemon-side create_session action failed; not forwarding to TUI` warnings? A failing `open_session` filters the action out before it reaches the TUI.
- in local (non-daemon) mode, are you aware that `create_session` / `switch_session` are **no-ops**? The hooker still runs (so your logging side-effects still happen), but the actions are dropped at the TUI's `switch_to_remote_session` because `remote_base_url()` is None — you will see a `tracing::warn!` reading `switch_to_remote_session called without remote backend; hook action dropped (local mode does not execute daemon-side actions)`. To exercise create/switch actions, run the daemon and connect the TUI in remote mode (see `docs/remote_tui.md`).
