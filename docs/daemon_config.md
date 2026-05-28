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
| `--host <HOST>` | Bind address | `0.0.0.0` |
| `--port <PORT>` | Listen port | `18080` |

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
app_id = "cli_..."
app_secret_env = "FEISHU_APP_SECRET"
verification_token = "your-token"
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

---

### [agents] - Multi-Agent Management

```toml
[agents]
id = "main"                          # Agent ID
default = true                       # Mark as default agent
model = "z-ai/glm-5"                 # Optional, override global model
system_prompt = "You are..."         # Optional, override default system prompt
workspace = "/path/to/workspace"     # Optional, workspace directory
```

---

### [paths] - Data Storage Paths

```toml
[paths]
data_dir = "~/.xiaoo"                # Optional, root directory for data storage
```

---

> **Note**: Common configuration items (llm, subagent, trace, compact, etc.) are shown in the "Complete Daemon Configuration Example" below. For detailed descriptions, please refer to [Configuration File Guide](./config_file_guide.md).

## Complete Daemon Configuration Example

Here is a complete example containing both common configuration and Daemon-specific configuration:

```toml
# Common configuration (applies to CLI/TUI/Daemon)
[llm]
provider = "openrouter"              # openai, anthropic, ollama, openrouter, deepseek, zai, minimax, kimi, minimax-coding-plan, kimi-coding-plan
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"
context_window = 128000

# Predefined subagent roles (common configuration) ⭐
[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.code_reviewer.tools]
bash = true
read = true
glob = true
grep = true

# Context compression (common configuration)
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

# Daemon-specific configuration
[agents]
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

#### `POST /api/v1/chat`

Chat endpoint. Send messages to the Gateway and receive responses.

**Request Body:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `text` | string | ✅ | Message text (must not be empty) |
| `channel` | string | *Either `channel` or `channel_instance_id`* | Channel identifier (e.g., `feishu`, `telegram`) |
| `channel_instance_id` | string | *Either `channel` or `channel_instance_id`* | Channel instance ID (for multi-instance session isolation) |
| `sender_id` | string | ✅ | Sender ID |
| `conversation_id` | string | ✅ | Conversation/group ID (same value reuses the same session) |
| `message_id` | string | — | Unique message ID (auto-generated if not specified) |
| `reply_to_message_id` | string | — | Target message ID being replied to |
| `root_message_id` | string | — | Root message ID of the thread |
| `mentions` | array | — | List of @mentions |

`mentions` element structure:

```json
{
  "id": "user-or-bot-id",
  "display_name": "Display name (optional)"
}
```

**Example Request:**

```bash
curl -X POST http://localhost:18080/api/v1/chat \
  -H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Hello",
    "channel": "test",
    "sender_id": "user-1",
    "conversation_id": "conv-demo",
    "mentions": [{"id": "bot", "display_name": "XiaoO"}]
  }'
```

**Response `200 OK`:**

```json
{
  "reply": "Hello! How can I help you?",
  "raw_reply": "Hello! How can I help you?",
  "conversation_id": "conv-demo",
  "session_id": "test:conv-demo"
}
```

| Field | Description |
|-------|-------------|
| `reply` | Final visible reply text (after post-processing) |
| `raw_reply` | Raw reply text |
| `conversation_id` | Conversation ID (same as request) |
| `session_id` | Internal session identifier (format: `{channel_or_instance}:{conversation_id}`) |

**Error Responses:**

- `400 Bad Request` — Missing required fields or validation failure:
  ```json
  { "error": "channel or channel_instance_id is required" }
  ```
  ```json
  { "error": "text must not be empty" }
  ```
- `500 Internal Server Error` — Session service internal error
- `401 Unauthorized` — Missing or invalid Bearer token when `[http]` auth is configured
- `429 Too Many Requests` — Rate limit exceeded when `[http.rate_limit]` is enabled

> **Rate limiting applies globally** to all endpoints (`/api/v1/health`, `/api/v1/chat`, `/api/v1/chat/stream`, `/api/v1/channels/{channel_id}/events`). Client identity is extracted from the `X-Forwarded-For` header (first IP) or `X-Real-Ip`, falling back to a shared `"unknown"` bucket. Ensure your reverse proxy (nginx / Caddy) forwards these headers.

---

#### `POST /api/v1/chat/stream`

Streaming chat endpoint. Same request format as `/api/v1/chat`, but returns a **Server-Sent Events (SSE)** stream with real-time updates for LLM text generation and tool execution.

**Request Body:** Same as `/api/v1/chat`.

**Response:** `200 OK`, Content-Type `text/event-stream`

**SSE Event Types:**

| Event | Fields | Description |
|-------|--------|-------------|
| `turn_start` | `agent_id`, `turn` | Emitted at the start of each agent loop turn |
| `text_delta` | `delta`, `snapshot` | Emitted for each LLM text chunk. `delta` is the incremental text, `snapshot` is the cumulative text so far |
| `tool_result` | `call_id`, `tool_name`, `output_preview`, `is_error` | Emitted after each tool execution completes |
| `done` | `reply`, `raw_reply`, `conversation_id`, `session_id`, `turn_count`, `total_tokens`, `stop_reason` | Emitted when the agent loop finishes. Stream closes after this event |
| `error` | `error` | Emitted on failure. Stream closes after this event |

**Example Request:**

```bash
curl -N -X POST http://localhost:18080/api/v1/chat/stream \
  -H "Authorization: Bearer $XIAOO_HTTP_BEARER_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "text": "Hello",
    "channel": "test",
    "sender_id": "user-1",
    "conversation_id": "conv-demo"
  }'
```

**Example SSE Output:**

```
event: turn_start
data: {"type":"turn_start","agent_id":"main","turn":1}

event: text_delta
data: {"type":"text_delta","delta":"Hello","snapshot":"Hello"}

event: text_delta
data: {"type":"text_delta","delta":"! How can I help you?","snapshot":"Hello! How can I help you?"}

event: done
data: {"type":"done","reply":"Hello! How can I help you?","raw_reply":"Hello! How can I help you?","conversation_id":"conv-demo","session_id":"test:conv-demo","turn_count":1,"total_tokens":150,"stop_reason":"complete"}
```

**Error Responses:**

- `400 Bad Request` — Same validation errors as `/api/v1/chat`
- `401 Unauthorized` — Missing or invalid Bearer token when `[http]` auth is configured
- `429 Too Many Requests` — Rate limit exceeded when `[http.rate_limit]` is enabled

**429 Response:**

```json
{ "error": "rate limit exceeded; retry after 1s" }
```

| Header | Description |
|--------|-------------|
| `Retry-After` | Seconds until quota resets |
| `X-RateLimit-Remaining` | Remaining requests (always `0` when 429) |

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
