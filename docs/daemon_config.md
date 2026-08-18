# Daemon Configuration Guide

> **Note**: This document focuses on Daemon-specific configuration items.
>
> For **common configuration items** (llm, subagent, skills, compact, trace, hooker, etc.), please refer to:
> - [Configuration File Guide](./config_file_guide.md) - Detailed common configuration
> - [CLI Configuration](./cli_config.md) - CLI basic usage
> - [TUI Configuration](./tui_config.md) - TUI-specific configuration

---

## Daemon Startup Parameters

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--config <PATH>` | Path to configuration file (also supports `XIAOO_CONFIG` environment variable, falling back to `~/.config/xiaoo/config.toml`) | Auto-detect |
| `--host <HOST>` | Bind address for the runtime API | `0.0.0.0` |
| `--port <PORT>` | Listen port for the runtime API | `18080` |
| `--dashboard-host <HOST>` | Bind address for the read-only session/sandbox dashboard | `127.0.0.1` |
| `--dashboard-port <PORT>` | Listen port for the dashboard. If the port is already in use the daemon automatically tries the next one (28082, 28083, …) up to 100 attempts. | `28081` |

When the dashboard starts, the daemon prints and logs the resolved address, e.g.:

```
dashboard ready at http://127.0.0.1:28081
```

Open that URL in a browser to inspect every session and sandbox the daemon is currently tracking. The page auto-refreshes every 5 seconds.

---

## Daemon-specific Configuration Items

The following configuration items only take effect in Daemon mode:

### [channels] - Channel Integration

Channel integration allows Daemon to receive user requests through enterprise IM like Feishu, Telegram, etc.

Detailed deployment guides:
- Feishu integration: [feishu_deploy.md](./feishu_deploy.md)
- Telegram integration: [telegram_deploy.md](./telegram_deploy.md)

#### Feishu Configuration

```toml
[channels.feishu]
enabled = true
channel_instance_id = "ops-feishu"   # Optional, defaults to "feishu"
transport = "webhook"                # webhook (default) | websocket
app_id = "cli_..."
app_secret_env = "FEISHU_APP_SECRET"
verification_token = "your-token"    # Required for webhook mode; optional for websocket
base_url = "https://open.feishu.cn"  # Optional, default value
```

#### Telegram Configuration

```toml
[channels.telegram]
enabled = true
channel_instance_id = "ops-telegram" # Optional, defaults to "telegram"
transport = "webhook"               # webhook (default) | polling
bot_token_env = "TELEGRAM_BOT_TOKEN" # Required, Telegram Bot API token env var
webhook_secret_token = "your-token"  # Webhook only; must match X-Telegram-Bot-Api-Secret-Token
bot_username = "@xiaoO_bot"          # Optional, strips leading @bot or /cmd@bot invocations
base_url = "https://api.telegram.org" # Optional, default value
polling_timeout_secs = 50           # Polling only; Bot API getUpdates timeout
polling_limit = 100                 # Polling only; 1-100 updates per request
```

---

### [http] - HTTP API Configuration

#### Bearer Authentication

```toml
[http]
bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN"
# bearer_token = "local-dev-token"   # Optional, use env var in production; do not set both
```

#### Rate Limiting

```toml
[http.rate_limit]
enabled = true                      # Enable or disable rate limiting; default: true
requests_per_second = 2             # Default refill rate; default: 2 (≈120 req/min)
burst = 10                          # Max burst size; default: 10

# Per-route overrides (optional)
# [http.rate_limit.routes.health]
# requests_per_second = 10          # Health checks get a wider quota
# burst = 30

# [http.rate_limit.routes.chat]
# requests_per_second = 1           # Chat API is the most expensive endpoint
# burst = 5
```

#### Dashboard

The dashboard is a read-only web UI served on its own port so it never
shares the runtime API's bearer auth. Open `http://<dashboard-host>:<dashboard-port>/`
in a browser to inspect every session, every sandbox (operation backend) and
the link between them. The page auto-refreshes every 5 seconds; no actions
are exposed.

```toml
[http.dashboard]
enabled = true                # Optional; default true. Set false to skip starting the dashboard server.
host = "127.0.0.1"            # Optional; default 127.0.0.1. Set 0.0.0.0 to expose externally.
port = 28081                  # Optional; default 28081. On conflict the daemon walks the next port up to 100 times.
```

CLI flags `--dashboard-host` and `--dashboard-port` take precedence over the
config-file values. The dashboard never requires a bearer token regardless of
the `[http]` auth configuration; bind to `127.0.0.1` (the default) or front it
with a reverse proxy if you need access control.

---

### [agents] - Multi-Agent Management

```toml
[agents]
default_agent_id = "main"             # Optional, default agent id

[[agents.list]]
id = "main"                           # Agent ID
default = true                        # Mark as default agent
model = "z-ai/glm-5"                  # Optional, override global model
system_prompt = "You are..."          # Optional, override default system prompt
workspace = "/path/to/workspace"      # Optional, workspace directory
```

---

### [paths] - Data Storage Paths

```toml
[paths]
data_dir = "~/.xiaoo"                # Optional, root directory for data storage
```

---

### [server.operation_backend] - Daemon Operation Backend

The daemon reads operation backend configuration only from the `server`
namespace. Top-level `[operation_backend]` is reserved for CLI/TUI-side
configuration and is ignored by the daemon.

```toml
[server.operation_backend]
kind = "e2b"

[server.operation_backend.options]
api_key_env = "E2B_API_KEY"          # Or api_key = "..."
template_id = "base"
timeout_secs = 3600
secure = true
workspace_root = "/home/user/workspace"
home_dir = "/home/user"
temp_root = "/tmp"
default_shell = "/bin/sh"
```

For a self-hosted E2B deployment, configure both the control-plane API URL and
the sandbox domain. `domain` is a hostname and must not include a URL scheme or
path.

```toml
[server.operation_backend]
kind = "e2b"

[server.operation_backend.options]
api_key_env = "E2B_API_KEY"
api_base = "https://api.e2b.example.com"
domain = "e2b.example.com"
template_id = "base"
timeout_secs = 3600
secure = true
```

The connection settings use the following precedence:

- Control-plane API: `api_base` (also accepts `api_url`) → `E2B_API_URL` →
  `https://api.<domain>`.
- Sandbox domain: `domain` → `E2B_DOMAIN` → `e2b.app`.

For example, the equivalent environment-based configuration is:

```bash
export E2B_API_KEY="<self-hosted-api-key>"
export E2B_API_URL="https://api.e2b.example.com"
export E2B_DOMAIN="e2b.example.com"
```

The daemon process must inherit these environment variables. A shell export
does not change the environment of an already-running daemon.

Live E2B sandbox limits are shared by all xiaoO processes running as the same
Unix user. Configure the per-provider-key limit in the separate global sandbox
configuration file. If the file or field is absent, xiaoO defaults to 20 live
sandboxes per key. Paused runtimes and provider checkpoint templates do not
count toward this limit.

```toml
# ~/.config/xiaoo/sandbox.toml
max_sandbox_cnt = 20
```

The current daemon does not read `[server.resource_limits]`; that older section
must not be used for sandbox limits. Runtime processes coordinate confirmed and
in-progress sandbox counts through `~/.xiaoo/sandbox_counts.json` and record
backend ownership/activity in `~/.xiaoo/backend_registry.json`. Provider API
keys are stored only as derived identifiers in these shared files, not as
plaintext.

Use the local backend in daemon mode by setting `kind = "local"` under the same
`[server.operation_backend]` namespace.

```toml
[server.operation_backend]
kind = "local"

[server.operation_backend.options.isolation]
kind = "linux_bubblewrap"
allow_network = false
```

---

> **Note**: Common configuration items (llm, subagent, trace, compact, etc.) are shown in the "Complete Daemon Configuration Example" below. For detailed descriptions, please refer to [Configuration File Guide](./config_file_guide.md).

## Complete Daemon Configuration Example

Here is a complete example containing both common configuration and Daemon-specific configuration:

```toml
# Common configuration (applies to CLI/TUI/Daemon)
[llm]
provider = "openrouter"              # openai, anthropic, gemini, ollama, openrouter, deepseek, zai, groq, mistral, together, xai, minimax, kimi, gitcode, local, ... (see config_file_guide.md)
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"
max_tokens = 128000
# Note: daemon's [llm] does NOT read reasoning_effort; pass it per-turn via the
# HTTP API `reasoning_effort` field in RuntimeTurnRequest.

# Predefined subagent roles (common configuration)
# Note: Tools configuration supports two formats. See config_file_guide.md for details.
[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
read = true
glob = true
grep = true

# Context compression (CLI/Daemon only; TUI ignores [compact]).
# This section is OPTIONAL: omit it to use the built-in defaults
# (warning=0.6 / auto_compact=0.75 / blocking=0.9). Compression is never
# silently disabled — a missing section yields a real ContextManager.
[compact]
auto_compact_ratio = 0.75

# Tracing (common configuration)
[trace]
storage_backend = "moirai-sqlite"
db_path = "~/.xiaoo/traces.db"

# Skills (common configuration)
[skills]
dirs = ["~/.xiaoo/skills"]

# Hooker (common configuration)
[hooker]
default = "audit_agent"

# Encrypted secrets storage (common configuration; read via xiaoo_shared::llm_secrets)
[vault]
enabled = false
use_sdf = false

# Daemon-specific configuration
[agents]
default_agent_id = "main"

[[agents.list]]
id = "main"
default = true
model = "z-ai/glm-5"

# HTTP API configuration (Daemon-specific)
[http]
bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN"

[http.rate_limit]
enabled = true
requests_per_second = 2
burst = 10

# Feishu integration (Daemon-specific)
[channels.feishu]
enabled = true
channel_instance_id = "ops-feishu"
transport = "webhook"
app_id = "cli_..."
app_secret_env = "FEISHU_APP_SECRET"
verification_token = "your-token"

# Telegram integration (Daemon-specific)
[channels.telegram]
enabled = true
channel_instance_id = "ops-telegram"
transport = "webhook"
bot_token_env = "TELEGRAM_BOT_TOKEN"
webhook_secret_token = "your-token"

# Data storage path (Daemon-specific)
[paths]
data_dir = "~/.xiaoo"

# Operation backend (Daemon-specific)
[server.operation_backend]
kind = "e2b"

[server.operation_backend.options]
api_key_env = "E2B_API_KEY"
template_id = "base"
timeout_secs = 3600
```

Set the shared E2B sandbox limit separately in
`~/.config/xiaoo/sandbox.toml`:

```toml
max_sandbox_cnt = 20
```

### API Endpoints

#### `GET /api/v1/health`

Health check endpoint for liveness probes and load balancing.

**Response `200 OK`:**

```json
{
  "status": "ok",
  "version": "0.1.0"
}
```

---

#### Session And Runtime Control Plane

The daemon exposes runtime APIs for remote TUI and other first-class clients,
plus checkpoint APIs for callers that need branching execution state.
These endpoints are protected by HTTP Bearer auth when `[http]` auth is configured.
LLM provider settings are resolved per runtime/turn: request payloads may pass an
optional `llm` object, and omitted fields fall back to `[llm]` in the daemon
config. The daemon does not require the LLM API key at process startup.

| Endpoint | Description |
|----------|-------------|
| `POST /api/v1/runtimes/open` | Open or resume a runtime using `RuntimeOpenRequest` |
| `POST /api/v1/runtimes/input` | Submit one user input and stream SSE events |
| `POST /api/v1/runtimes/interaction` | Send a user interaction response back to the daemon |
| `POST /api/v1/runtimes/cancel` | Request cancellation of the current turn |
| `POST /api/v1/runtimes/close` | Close the runtime, remove its record, and fire lifecycle hooks |
| `POST /api/v1/runtimes/checkpoint` | Capture an idle runtime as a checkpoint using `RuntimeCheckpointRequest` |
| `POST /api/v1/runtimes/checkpoint/delete-snapshot` | Delete the provider snapshot/template referenced by a checkpoint |
| `POST /api/v1/runtimes/checkout` | Create a new runtime from a checkpoint using `RuntimeCheckoutRequest` |
| `POST /api/v1/runtimes/pause` | Snapshot an idle runtime and release its live backend (`RuntimePauseRequest`) |
| `POST /api/v1/runtimes/resume` | Restore a paused runtime with the same runtime id (`RuntimeResumeRequest`) |
| `POST /api/v1/runtimes/exec` | Run a shell command inside the runtime's backend (`RuntimeExecRequest`) |
| `POST /api/v1/runtimes/read-file` | Read a file from the runtime's backend (`RuntimeReadFileRequest`) |
| `POST /api/v1/runtimes/write-file` | Write a file inside the runtime's backend (`RuntimeWriteFileRequest`) |

Runtime APIs use `runtime_id` and `checkpoint_id` as the public vocabulary. In
the current v1 implementation, `runtime_id` is backed by the same value as the
internal `session_id`; backend ids remain internal and are not returned in
`RuntimeRecord`. See [Runtime Checkpoint Control](./runtime_checkpoint.md)
for the current layering and checkpoint semantics.

**Open runtime example:**

```bash
curl -X POST http://localhost:18080/api/v1/runtimes/open \
  -H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "runtime_id": "tui-demo",
    "conversation_id": "conv-demo",
    "sender_id": "user-1",
    "entry": { "kind": "tui" },
    "channel": "tui",
    "llm": {
      "provider": "deepseek",
      "model": "deepseek-v4-pro",
      "api_key_env": "DEEPSEEK_API_KEY"
    }
  }'
```

**Close runtime example:**

```bash
curl -X POST http://localhost:18080/api/v1/runtimes/close \
  -H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "runtime_id": "tui-demo"
  }'
```

**Submit input stream example:**

```bash
curl -N -X POST http://localhost:18080/api/v1/runtimes/input \
  -H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "runtime_id": "tui-demo",
    "entry": { "kind": "tui" },
    "channel": "tui",
    "conversation_id": "conv-demo",
    "sender_id": "user-1",
    "text": "Hello",
    "mentions": [],
    "llm": {
      "provider": "openai",
      "model": "gpt-4o",
      "api_key_env": "OPENAI_API_KEY"
    }
  }'
```

**Runtime checkpoint / checkout example with timing:**

The runtime checkpoint APIs require the source runtime to be idle. In the
current v1 implementation, the `runtime_id` is the same value as the runtime id
returned by `/api/v1/runtimes/open`. The following flow creates a runtime, runs
one turn to make the backend dirty, captures a checkpoint, checks out a child
runtime, runs both branches, and closes both runtimes.

The example requires Bash, `curl`, and `jq`. It only adds the `Authorization`
header when `XIAOO_HTTP_BEARER_TOKEN` is present. It uses
`curl -w '%{time_total}'` to capture end-to-end HTTP request latency for the
checkpoint and checkout control-plane calls.

```bash
BASE_URL="http://localhost:18080"
RUNTIME="checkpoint-demo-$(date +%Y%m%d%H%M%S)"
CONV="conv-${RUNTIME}"
SENDER="checkpoint-demo-user"

AUTH_HEADER=()
if [ -n "${XIAOO_HTTP_BEARER_TOKEN:-}" ]; then
  AUTH_HEADER=(-H "Authorization: Bearer ${XIAOO_HTTP_BEARER_TOKEN}")
fi

jq -n --arg runtime "$RUNTIME" --arg conv "$CONV" --arg sender "$SENDER" \
  '{
    runtime_id: $runtime,
    conversation_id: $conv,
    sender_id: $sender,
    entry: { kind: "http_api", instance_id: "checkpoint-demo" }
  }' > /tmp/xiaoo_open.json

curl -sS -X POST "$BASE_URL/api/v1/runtimes/open" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_open.json \
  > /tmp/xiaoo_open.out
```

```bash
INIT_TEXT="请在当前 agent runtime 的工作区创建文件 /home/user/workspace/checkpoint_demo.txt，内容为两行：第一行 checkpoint base，第二行 runtime parent initialized。完成后读取该文件并回复其完整内容。"

jq -n \
  --arg runtime "$RUNTIME" \
  --arg conv "$CONV" \
  --arg sender "$SENDER" \
  --arg text "$INIT_TEXT" \
  '{
    runtime_id: $runtime,
    entry: { kind: "http_api", instance_id: "checkpoint-demo" },
    channel: null,
    message_id: null,
    conversation_id: $conv,
    sender_id: $sender,
    text: $text,
    channel_instance_id: null,
    channel_identity_prompt: null,
    reply_to_message_id: null,
    root_message_id: null,
    mentions: [],
    reasoning_effort: "off",
    llm: null
  }' > /tmp/xiaoo_initial_turn.json

curl -sS -N -X POST "$BASE_URL/api/v1/runtimes/input" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_initial_turn.json \
  > /tmp/xiaoo_initial_turn.sse
```

```bash
jq -n --arg runtime "$RUNTIME" \
  '{
    runtime_id: $runtime,
    name: "fork-test-base",
    metadata: {
      purpose: "checkpoint checkout smoke test",
      requested_line: "测试fork"
    }
  }' > /tmp/xiaoo_checkpoint.json

curl -sS \
  -o /tmp/xiaoo_checkpoint.out \
  -w "%{time_total}\n" \
  -X POST "$BASE_URL/api/v1/runtimes/checkpoint" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_checkpoint.json \
  > /tmp/xiaoo_checkpoint.time

CHECKPOINT_ID="$(jq -r '.checkpoint_id' /tmp/xiaoo_checkpoint.out)"
printf "checkpoint_time_total_seconds=%s\n" "$(cat /tmp/xiaoo_checkpoint.time)"
```

```bash
jq -n \
  --arg checkpoint "$CHECKPOINT_ID" \
  --arg child_conv "conv-${RUNTIME}-child" \
  '{
    checkpoint_id: $checkpoint,
    conversation_id: $child_conv,
    sender_id: "checkpoint-demo-child",
    metadata: {
      branch: "child",
      requested_line: "测试fork"
    }
  }' > /tmp/xiaoo_checkout.json

curl -sS \
  -o /tmp/xiaoo_checkout.out \
  -w "%{time_total}\n" \
  -X POST "$BASE_URL/api/v1/runtimes/checkout" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_checkout.json \
  > /tmp/xiaoo_checkout.time

CHILD_RUNTIME="$(jq -r '.runtime.runtime_id' /tmp/xiaoo_checkout.out)"
printf "checkout_time_total_seconds=%s\n" "$(cat /tmp/xiaoo_checkout.time)"
```

If a checkpoint's provider snapshot is no longer needed for future checkout,
delete it explicitly. The checkpoint record remains in daemon memory for lineage
metadata, but after this call it no longer has an E2B provider snapshot and
cannot be used to create another checkout branch.

```bash
jq -n --arg checkpoint "$CHECKPOINT_ID" \
  '{ checkpoint_id: $checkpoint }' \
  > /tmp/xiaoo_delete_snapshot.json

curl -sS -X POST "$BASE_URL/api/v1/runtimes/checkpoint/delete-snapshot" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_delete_snapshot.json
```

```bash
PARENT_TEXT="你是父 runtime。请不要写入“测试fork”。请在 /home/user/workspace/checkpoint_demo.txt 末尾追加一行：parent runtime complete。完成后读取该文件并回复完整内容。"
CHILD_TEXT="你是 checkpoint checkout 出来的子 runtime。请在 /home/user/workspace/checkpoint_demo.txt 末尾追加一行：测试fork。完成后读取该文件并回复完整内容。"

jq -n --arg runtime "$RUNTIME" --arg conv "$CONV" --arg text "$PARENT_TEXT" \
  '{
    runtime_id: $runtime,
    entry: { kind: "http_api", instance_id: "checkpoint-demo" },
    channel: null,
    message_id: null,
    conversation_id: $conv,
    sender_id: "checkpoint-demo-user",
    text: $text,
    channel_instance_id: null,
    channel_identity_prompt: null,
    reply_to_message_id: null,
    root_message_id: null,
    mentions: [],
    reasoning_effort: "off",
    llm: null
  }' > /tmp/xiaoo_parent_final.json

curl -sS -N -X POST "$BASE_URL/api/v1/runtimes/input" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_parent_final.json \
  > /tmp/xiaoo_parent_final.sse

jq -n --arg runtime "$CHILD_RUNTIME" --arg text "$CHILD_TEXT" \
  '{
    runtime_id: $runtime,
    entry: { kind: "http_api", instance_id: "checkpoint-demo-child" },
    channel: null,
    message_id: null,
    conversation_id: "conv-checkpoint-demo-child",
    sender_id: "checkpoint-demo-child",
    text: $text,
    channel_instance_id: null,
    channel_identity_prompt: null,
    reply_to_message_id: null,
    root_message_id: null,
    mentions: [],
    reasoning_effort: "off",
    llm: null
  }' > /tmp/xiaoo_child_final.json

curl -sS -N -X POST "$BASE_URL/api/v1/runtimes/input" \
  "${AUTH_HEADER[@]}" \
  -H "Content-Type: application/json" \
  --data @/tmp/xiaoo_child_final.json \
  > /tmp/xiaoo_child_final.sse
```

```bash
for id in "$RUNTIME" "$CHILD_RUNTIME"; do
  jq -n --arg runtime "$id" '{ runtime_id: $runtime }' \
    > "/tmp/xiaoo_close_${id//[^A-Za-z0-9_]/_}.json"
  curl -sS -X POST "$BASE_URL/api/v1/runtimes/close" \
    "${AUTH_HEADER[@]}" \
    -H "Content-Type: application/json" \
    --data @"/tmp/xiaoo_close_${id//[^A-Za-z0-9_]/_}.json"
done
```

With E2B as `[server.operation_backend]`, a local smoke run on 2026-06-13
measured:

| Operation | Measured `curl` `time_total` | E2B work included |
|-----------|-------------------------------|-------------------|
| Runtime checkpoint | `1.516061s` | Create an E2B provider snapshot for the dirty parent sandbox |
| Runtime checkout | `2.335461s` | Start a new E2B sandbox from the provider snapshot and bind it to the child runtime |

These values are examples, not guarantees. They vary with E2B provider latency,
network path, snapshot size, template cold/warm state, and daemon host load. The
numbers above do not include the LLM turns before or after the checkpoint, and
they do not include closing the runtimes. Closing an E2B-backed runtime calls
backend release, which deletes the corresponding E2B sandbox.

**SSE Event Types:**

All events are tagged with `type` and emit a matching SSE `event:` name. `agent_id` is included on streaming events so multi-agent runtimes can be rendered separately.

| Event | Fields | Description |
|-------|--------|-------------|
| `turn_start` | `agent_id`, `turn` | Emitted at the start of each agent loop turn |
| `text_delta` | `agent_id`, `delta`, `snapshot` | Emitted for assistant text updates |
| `thinking_delta` | `agent_id`, `delta`, `snapshot` | Emitted for assistant reasoning updates |
| `tool_result` | `agent_id`, `call_id`, `tool_name`, `output_preview`, `is_error` | Emitted after each tool execution completes |
| `interaction_requested` | `request` | Emitted when the daemon needs a user confirmation/input/choice |
| `done` | `reply`, `raw_reply`, `conversation_id`, `runtime_id`, `turn_count`, `total_tokens`, `prompt_tokens`, `completion_tokens`, `estimated_input_tokens`, `messages`, `stop_reason` | Emitted when the agent loop finishes |
| `error` | `error` | Emitted on failure |
| `cancelled` | `runtime_id` | Emitted as cancellation acknowledgement |

> The `done` and `cancelled` events serialize the runtime handle as `runtime_id`
> in the JSON body even though the internal field is `session_id`; this is the
> public vocabulary used by `RuntimeRecord`.

**Common Error Responses:**

- `400 Bad Request` — malformed request or path/runtime mismatch
- `401 Unauthorized` — missing or invalid Bearer token when `[http]` auth is configured
- `404 Not Found` — runtime not found
- `429 Too Many Requests` — rate limit exceeded when `[http.rate_limit]` is enabled
- `500 Internal Server Error` — runtime service internal error

> **Rate limiting applies globally** to all endpoints (`/api/v1/health`, `/api/v1/runtimes/*`, `/api/v1/channels/{channel_id}/events`). Client identity is extracted from the `X-Forwarded-For` header (first IP) or `X-Real-Ip`, falling back to a shared `"unknown"` bucket. Ensure your reverse proxy (nginx / Caddy) forwards these headers.

---

#### `POST /api/v1/channels/{channel_id}/events`

Channel event callback endpoint. Only available when the matching channel configuration is enabled in Daemon configuration.

**Behavior:**

- **URL Verification**: When Feishu platform first configures Webhook, it sends a challenge request; Gateway returns `{ "challenge": "..." }` as-is to complete verification.
- **Message Event Handling**: Upon receiving Feishu message events, Gateway processes asynchronously (returns ack immediately when `requires_async_processing=true`), and sends replies back to the original conversation via Feishu API.
- **Member Directory Injection**: Automatically loads group member list before processing and injects `<participant_directory>` into system prompt, enabling AI to perceive conversation participant identities.
- **Telegram Message Handling**: Telegram `message` and `channel_post` text updates are converted into the same internal `ChannelMessage` shape and replied to with Bot API `sendMessage`.
- **Telegram Polling Mode**: When `[channels.telegram].transport = "polling"`, Telegram events are received through Bot API `getUpdates` from an outbound daemon task instead of this HTTP callback endpoint. Telegram Bot API provides webhook and `getUpdates`; it does not provide a Bot API WebSocket transport.

**Request:**

Called by the channel platform via POST. Body is the raw JSON event payload. Headers contain the channel's own verification material.

**Response:**

- **Challenge verification**: `200 OK` → `{ "challenge": "<token>" }`
- **Message received**: `200 OK` → `{ "code": 0, "message": "ok" }`
- **Channel not configured**: `503 Service Unavailable` → `{ "error": "<channel_id> webhook is not configured" }`

Feishu callback URL:

```text
http://<your-host>:<port>/api/v1/channels/feishu/events
```

Telegram callback URL:

```text
https://<your-host>/api/v1/channels/telegram/events
```

When `webhook_secret_token` is configured, set the same value in Telegram `setWebhook.secret_token`; Telegram will send it in `X-Telegram-Bot-Api-Secret-Token`.

> This endpoint is intentionally **not** wrapped by the HTTP Bearer middleware; channel requests use each platform's own verification flow.

### Session Isolation Mechanism

Gateway implements session isolation via **session_id**:

```
session_id = "{channel_instance_id or channel}:{conversation_id}"
```

- Same `(channel, conversation_id)` combination shares the same session (retains context history).
- Different `conversation_id` creates independent sessions.
- When `channel_instance_id` is configured, it is used as prefix (supports multi-instance deployment of same channel type, e.g., multiple Feishu or Telegram bots).

### Channel Interaction Timeout

When the agent needs to ask the user a question (via `ask_user_question` tool), it sends the question to the channel (e.g., Feishu or Telegram) and waits for the user's reply. If the user does not reply within the configured timeout, the interaction is cancelled.

```toml
[channels]
interaction_timeout_secs = 600   # Timeout in seconds, default: 600 (10 minutes)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `interaction_timeout_secs` | integer | `600` | Maximum seconds to wait for a user reply. The value is rounded **up** to the nearest whole minute (minimum 1 minute). For example, `10` → 1 minute, `90` → 2 minutes, `600` → 10 minutes. Both the actual timeout and the displayed prompt use the rounded value. When the timeout expires, the pending interaction is cancelled, the user is notified, and the agent stops the current task. |
