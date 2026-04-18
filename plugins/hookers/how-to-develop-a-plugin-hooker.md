# How to develop a plugin hooker

This guide explains how to create a plugin hooker without touching Rust code in the `hooker` crate.

## 0. The simplest way

Add your hooker under `<your_xiaoO>/plugins/hookers`. A subdirectory is recognized as a valid hooker only if it contains a `plugin.json` (refers to plugin.json-example). For file configuration details, see the sections below. If your hooker needs extra setup, add an `install.sh` in the hooker directory — it will be executed automatically. After adding a hooker, run `<your_xiaoO>/plugins/hookers/config.sh` manually and follow the prompts to configure.

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

For tool hooks today:

- `action` must be `Tool`
- `stage` must be `pre`, `post`, or `error`

Examples:

- `tool_cli.Tool.file_read.pre`
- `cli-agent.Tool.glob.post`
- `*.Tool.*.pre`

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
