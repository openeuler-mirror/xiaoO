# Daemon Document

| Parameter | Description | Default |
|-----------|-------------|---------|
| `--config <PATH>` | Path to configuration file (also supports `XIAOO_CONFIG` environment variable, falling back to `~/.config/xiaoo/config.toml`) | Auto-detect |
| `--host <HOST>` | Bind address | `0.0.0.0` |
| `--port <PORT>` | Listen port | `18080` |

### Daemon Configuration File (TOML)

```toml
[llm]
provider = "openrouter"              # openai, anthropic, ollama, openrouter, deepseek, zai, ...
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"   # Read API key from this environment variable
api_base = "https://..."             # Optional, custom API base URL
context_window = 128000              # Optional, session compression budget
max_tokens = 128000                   # Optional, max tokens per response

[channels.feishu]                   # Optional, enable Feishu channel integration
enabled = true
app_id = "cli_..."
app_secret_env = "FEISHU_APP_SECRET"
verification_token = "your-token"
base_url = "https://open.feishu.cn"  # Optional, default value

[http]                               # Optional, enable Bearer auth for chat APIs
bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN"
# bearer_token = "local-dev-token"   # Optional, use env var in production; do not set both

[http.rate_limit]                   # Optional, per-IP rate limiting for all HTTP endpoints
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

[trace]                              # Optional, tracing/observability config
storage_backend = "moirai-sqlite"    # moirai-sqlite (default) | stdout | noop
db_path = "~/.xiaoo/traces.db"      # Database path for moirai-sqlite; uses trace crate built-in default if not configured

[compact]                            # Optional, context compression strategy
warning_ratio = 0.6                  # History ratio to enter warning stage
auto_compact_ratio = 0.75            # History ratio to trigger auto-compact
blocking_ratio = 0.9                 # History ratio to enter blocking stage
summary_max_tokens = 1024            # Max token budget for summary
summary_preserve_tail = 4            # Number of recent messages to preserve after summary
snip_stale_after_ms = 3600000        # History snip timeout (milliseconds)

[agents]
id = "main"                          # Agent ID
default = true                       # Mark as default agent
model = "z-ai/glm-5"                 # Optional, override global model
system_prompt = "You are..."         # Optional, override default system prompt
workspace = "/path/to/workspace"     # Optional, workspace directory

[paths]
data_dir = "~/.xiaoo"                # Optional, root directory for data storage
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
| `channel` | string | *Either `channel` or `channel_instance_id`* | Channel identifier (e.g., `feishu`, `dingtalk`) |
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

> **Rate limiting applies globally** to all endpoints (`/api/v1/health`, `/api/v1/chat`, `/api/v1/chat/stream`, `/api/v1/channels/feishu/events`). Client identity is extracted from the `X-Forwarded-For` header (first IP) or `X-Real-Ip`, falling back to a shared `"unknown"` bucket. Ensure your reverse proxy (nginx / Caddy) forwards these headers.

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

#### `POST /api/v1/channels/feishu/events`

Feishu event callback endpoint. Only available when `[channels.feishu]` is enabled in Daemon configuration.

**Behavior:**

- **URL Verification**: When Feishu platform first configures Webhook, it sends a challenge request; Gateway returns `{ "challenge": "..." }` as-is to complete verification.
- **Message Event Handling**: Upon receiving Feishu message events, Gateway processes asynchronously (returns ack immediately when `requires_async_processing=true`), and sends replies back to the original conversation via Feishu API.
- **Member Directory Injection**: Automatically loads group member list before processing and injects `<participant_directory>` into system prompt, enabling AI to perceive conversation participant identities.

**Request:**

Called by Feishu platform via POST, Body is raw JSON event payload, Headers contain Feishu signature information.

**Response:**

- **Challenge verification**: `200 OK` → `{ "challenge": "<token>" }`
- **Message received**: `200 OK` → `{ "code": 0, "message": "ok" }`
- **Feishu not configured**: `503 Service Unavailable` → `{ "error": "feishu webhook is not configured" }`

> ⚠️ This endpoint requires Feishu Open Platform Event Subscription configuration, pointing the callback URL to `http://<your-host>:<port>/api/v1/channels/feishu/events`.
> It is intentionally **not** wrapped by the HTTP Bearer middleware; Feishu requests continue to use Feishu's own verification flow.

### Session Isolation Mechanism

Gateway implements session isolation via **session_id**:

```
session_id = "{channel_instance_id or channel}:{conversation_id}"
```

- Same `(channel, conversation_id)` combination shares the same session (retains context history).
- Different `conversation_id` creates independent sessions.
- When `channel_instance_id` is configured, it is used as prefix (supports multi-instance deployment of same channel type, e.g., multiple Feishu apps).

### Channel Interaction Timeout

When the agent needs to ask the user a question (via `ask_user_question` tool), it sends the question to the channel (e.g., Feishu) and waits for the user's reply. If the user does not reply within the configured timeout, the interaction is cancelled.

```toml
[channels]
interaction_timeout_secs = 600   # Timeout in seconds, default: 600 (10 minutes)
```

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `interaction_timeout_secs` | integer | `600` | Maximum seconds to wait for a user reply. The value is rounded **up** to the nearest whole minute (minimum 1 minute). For example, `10` → 1 minute, `90` → 2 minutes, `600` → 10 minutes. Both the actual timeout and the displayed prompt use the rounded value. When the timeout expires, the pending interaction is cancelled, the user is notified, and the agent stops the current task. |
