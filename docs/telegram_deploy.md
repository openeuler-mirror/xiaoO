# Telegram Channel Deployment Guide

This guide covers the two Telegram connection modes supported by xiaoO:

- `webhook`: Telegram sends HTTPS callbacks to xiaoO.
- `polling`: xiaoO calls Telegram Bot API `getUpdates` from an outbound daemon task.

The [Telegram Bot API](https://core.telegram.org/bots/api) exposes webhook delivery and `getUpdates` polling for bots; it does not provide a Bot API WebSocket transport. The polling mode is the official Telegram Bot API alternative for deployments that cannot expose a public callback endpoint.

## 1. Create the Bot

Create the bot in Telegram with BotFather:

1. Open `@BotFather`.
2. Send `/newbot`.
3. Pick the display name and username.
4. Save the token as an environment variable on the host running xiaoO:

```bash
export TELEGRAM_BOT_TOKEN='<bot-token-from-botfather>'
```

Optional but recommended:

- Send `/setprivacy` to BotFather and disable privacy mode if the bot must read all group messages.
- Add the bot to the target private chat, group, supergroup, or channel.
- Set `bot_username` in xiaoO config so leading `@bot` and `/command@bot` invocations are stripped before the agent sees the message.

## 2. Shared xiaoO Config

Both modes use the same LLM and Telegram identity fields:

```toml
[llm]
provider = "openrouter"
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"

[channels]
interaction_timeout_secs = 600

[channels.telegram]
enabled = true
channel_instance_id = "ops-telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"
bot_username = "@xiaoO_bot"
base_url = "https://api.telegram.org"
```

Run the daemon after adding the transport-specific fields below:

```bash
cargo run -p xiaoo-app --bin xiaoo-app -- daemon \
  --config /opt/xiaoo/config/config.toml \
  --host 127.0.0.1 \
  --port 18080
```

## 3. Mode A: Webhook

Webhook mode requires a public HTTPS URL that Telegram can reach.

Add these fields:

```toml
[channels.telegram]
enabled = true
transport = "webhook"
bot_token_env = "TELEGRAM_BOT_TOKEN"
webhook_secret_token = "replace-with-a-random-secret"
bot_username = "@xiaoO_bot"
```

The xiaoO callback path is:

```text
POST /api/v1/channels/telegram/events
```

If nginx terminates TLS, proxy that path to the daemon:

```nginx
location /api/v1/channels/telegram/events {
    proxy_pass http://127.0.0.1:18080/api/v1/channels/telegram/events;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

Register the webhook:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/setWebhook" \
  -H 'Content-Type: application/json' \
  -d '{
    "url": "https://example.com/api/v1/channels/telegram/events",
    "secret_token": "replace-with-a-random-secret",
    "allowed_updates": ["message", "channel_post"]
  }'
```

Verify Telegram accepted it:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Expected checks:

- `url` is the public xiaoO callback URL.
- `pending_update_count` does not keep increasing after you send a test message.
- The daemon logs a request to `/api/v1/channels/telegram/events`.
- Telegram receives a `sendMessage` reply from xiaoO.

## 4. Mode B: Polling

Polling mode does not need a public callback URL. xiaoO only needs outbound HTTPS access to `https://api.telegram.org` and the model provider.

First remove any existing webhook because Telegram `getUpdates` and webhook delivery are mutually exclusive:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/deleteWebhook" \
  -H 'Content-Type: application/json' \
  -d '{"drop_pending_updates": false}'
```

Add these fields:

```toml
[channels.telegram]
enabled = true
transport = "polling"
bot_token_env = "TELEGRAM_BOT_TOKEN"
bot_username = "@xiaoO_bot"
polling_timeout_secs = 50
polling_limit = 100
```

Start the daemon. It will log `starting telegram polling transport` and keep one `getUpdates` loop running for the bot.

Verify no webhook is configured:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Expected checks:

- `url` is empty.
- Sending a private or group text message to the bot causes the daemon to poll and process the update.
- Telegram receives a `sendMessage` reply from xiaoO.
- The HTTP callback endpoint is not required in this mode; health and local chat APIs may still be served by the daemon.

## 5. Message Semantics

- Text updates from `message` and `channel_post` are supported.
- Non-text updates are acknowledged by the adapter and ignored.
- Forum topic conversations are encoded as `chat_id:message_thread_id`, so each topic keeps its own session.
- Replies are sent to the same chat and, when present, the same forum topic.
- `@bot` and `/command@bot` prefixes are stripped only when `bot_username` is configured.

## 6. Troubleshooting

Use Bot API status first:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getMe"
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Common checks:

- `TELEGRAM_BOT_TOKEN` must be present in the daemon environment.
- Webhook mode requires a valid public HTTPS URL.
- Polling mode requires no webhook to be set.
- Group messages may require disabling BotFather privacy mode.
- `webhook_secret_token` in config must match the `secret_token` passed to `setWebhook`.
