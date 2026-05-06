# Telegram Channel Deployment Guide

This document explains how to deploy the xiaoO Telegram channel in a way that is reproducible from scratch.

It covers:

- how Telegram connects to xiaoO in webhook mode
- how xiaoO connects to Telegram in polling mode
- which config files are used by the daemon
- how to expose the webhook safely through nginx
- how to verify bot identity, webhook delivery, polling delivery, and reply behavior

The examples below use the same service layout as the Feishu deployment guide, but Telegram has one important platform difference: the [Telegram Bot API](https://core.telegram.org/bots/api) supports **webhook** and **getUpdates long polling**. It does **not** provide a Bot API WebSocket transport.

## 1. End-to-end Request Flow

xiaoO supports two Telegram transport modes.

### Transport 1: Webhook Push

Webhook mode is the production-friendly public callback mode:

```text
Telegram user sends a message
  -> Telegram sends HTTPS POST to the public webhook URL
  -> nginx receives the request on 443
  -> nginx forwards the request to xiaoO on 127.0.0.1:18080
  -> xiaoO handles /api/v1/channels/telegram/events
  -> xiaoO calls Telegram Bot API sendMessage to reply
```

### Transport 2: getUpdates Polling

Polling mode is the local-development and private-network mode:

```text
Telegram user sends a message
  -> Telegram stores the update for the bot
  -> xiaoO daemon calls Telegram Bot API getUpdates over outbound HTTPS
  -> xiaoO handles the update through the same channel runtime
  -> xiaoO calls Telegram Bot API sendMessage to reply
```

Webhook and polling are mutually exclusive for the same bot token. If a webhook is configured, Telegram will not deliver updates through `getUpdates`.

## 2. Prerequisites

- A Telegram account
- A Telegram bot created through `@BotFather`
- A Linux or macOS host where you can install and run `xiaoo-app daemon`
- Rust toolchain and Cargo available on that host, unless you already have a built binary
- Outbound network access from xiaoO to:
  - `https://api.telegram.org`
  - your model provider, for example OpenRouter
- For webhook mode only:
  - a public HTTPS domain reachable by Telegram
  - `nginx` or another reverse proxy available for public ingress
- For production service management:
  - `systemd` or an equivalent process manager

## 3. Deployment Modes

There are three practical deployment modes for Telegram integration.

### Mode A: Local Deployment with Polling

This is the recommended local development mode.

In this setup:

- xiaoO runs on your local machine
- xiaoO binds to `127.0.0.1:18080`
- no public callback URL is needed
- xiaoO receives messages by long polling Telegram Bot API `getUpdates`

Typical flow:

```text
Telegram
  <- outbound HTTPS getUpdates from local xiaoO
local xiaoO daemon
```

This is usually the easiest pattern if:

- you are testing on a laptop
- you do not have a public HTTPS domain
- you want a long-running local bot connection
- you do not need Telegram to call your machine directly

Important:

- delete any existing webhook before switching to polling
- keep the daemon running while testing
- the daemon still needs outbound access to Telegram and the model provider

### Mode B: Server Deployment with Webhook

This is the recommended production mode when you have a public domain.

In this setup:

- xiaoO runs on a server
- xiaoO binds to `127.0.0.1:18080`
- nginx exposes a public HTTPS webhook URL
- Telegram sends updates to nginx
- nginx forwards updates to the local daemon

Typical flow:

```text
Telegram
  -> https://<your-domain>/api/v1/channels/telegram/events
  -> nginx
  -> xiaoO daemon on 127.0.0.1:18080
```

### Mode C: Local Deployment with Public Webhook Relay

This mode is useful when you specifically want to test webhook behavior locally.

In this setup:

- xiaoO runs on your local machine
- Telegram still needs a public HTTPS callback URL
- a tunnel or relay forwards the public URL to local `127.0.0.1:18080`

Typical flow:

```text
Telegram
  -> public HTTPS tunnel or relay
  -> local xiaoO daemon
```

Choose this mode only when you need to validate webhook behavior. For normal local testing, polling is simpler and more faithful to Telegram's private-network deployment model.

## 4. Prepare Code and Build the Binary

If you only have the source code and no existing deployment, start from the repository first.

Example:

```bash
git clone <your-repo-url> /opt/xiaoo/src
cd /opt/xiaoo/src
git checkout telegram
cargo build -p xiaoo-app
```

After a successful build, the binary will usually be created at:

```text
target/debug/xiaoo-app
```

For a long-running service, install that binary to a stable runtime path:

```bash
mkdir -p /opt/xiaoo/bin
install -m 755 target/debug/xiaoo-app /opt/xiaoo/bin/xiaoo-app
```

If you prefer release builds:

```bash
cargo build -p xiaoo-app --release
install -m 755 target/release/xiaoo-app /opt/xiaoo/bin/xiaoo-app
```

## 5. Prepare Runtime Directories

Before writing config or creating a service, create the runtime layout explicitly.

Example:

```bash
mkdir -p /opt/xiaoo/bin
mkdir -p /opt/xiaoo/config
mkdir -p /opt/xiaoo/app
mkdir -p /opt/xiaoo/adt/skills
mkdir -p /var/lib/xiaoo/agents/main
```

Recommended layout:

```text
/opt/xiaoo/bin/xiaoo-app
/opt/xiaoo/config/config.toml
/opt/xiaoo/config/xiaoo.env
/opt/xiaoo/app
/opt/xiaoo/adt/skills
/var/lib/xiaoo/agents/main
```

You can adjust these paths, but the same values must be used consistently across:

- `config.toml`
- `xiaoo.env`
- `systemd`
- `nginx`
- deployment scripts

## 6. Create the Telegram Bot

Create the bot through Telegram's official `@BotFather`.

1. Open Telegram.
2. Search for `@BotFather`.
3. Send `/newbot`.
4. Pick a bot display name.
5. Pick a bot username. Telegram bot usernames usually end with `_bot`.
6. Store the returned token in a secret environment file, not in `config.toml`.

Record the following values:

| Field | Used For |
|---|---|
| Bot token | environment variable referenced by `channels.telegram.bot_token_env` |
| Bot username | `channels.telegram.bot_username` in `config.toml` |
| Webhook secret token | webhook mode request authentication |

Recommended mapping:

- Bot token -> `TELEGRAM_BOT_TOKEN` in `xiaoo.env`
- Bot username -> `channels.telegram.bot_username`
- Webhook secret -> `channels.telegram.webhook_secret_token`

If the bot must read all group messages, configure BotFather privacy mode:

```text
/setprivacy
```

For group-only mention behavior, privacy mode can remain enabled. For full group-message ingestion, disable it.

## 7. xiaoO Daemon Configuration

The daemon reads:

- config file:
  - `/opt/xiaoo/config/config.toml`
- environment file:
  - `/opt/xiaoo/config/xiaoo.env`

Do not put the Telegram bot token directly into `config.toml`.

### Shared `config.toml` Base

Both Telegram modes share the same LLM and bot identity settings.

```toml
[llm]
provider = "openrouter"
api_base = "https://openrouter.ai/api/v1"
model = "z-ai/glm-5"
api_key_env = "OPENROUTER_API_KEY"
max_tokens = 8192

[channels]
interaction_timeout_secs = 600

[channels.telegram]
enabled = true
channel_instance_id = "ops-telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"
bot_username = "@your_bot_username"
base_url = "https://api.telegram.org"

[agents]
default_agent_id = "main"

[[agents.list]]
id = "main"
default = true
workspace = "/opt/xiaoo/app"
agent_dir = "/var/lib/xiaoo/agents/main"

[skills]
dirs = ["/opt/xiaoo/adt/skills"]
```

### Example `xiaoo.env`

```bash
TELEGRAM_BOT_TOKEN=your-real-telegram-bot-token
OPENROUTER_API_KEY=your-real-model-key
```

Keep this file readable only by the service user:

```bash
chmod 600 /opt/xiaoo/config/xiaoo.env
```

## 8. Polling Mode Configuration

Polling mode receives updates through Telegram Bot API `getUpdates`.

Add these fields under `[channels.telegram]`:

```toml
[channels.telegram]
enabled = true
transport = "polling"
channel_instance_id = "ops-telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"
bot_username = "@your_bot_username"
base_url = "https://api.telegram.org"
polling_timeout_secs = 50
polling_limit = 100
```

Before starting polling, remove any existing webhook:

```bash
source /opt/xiaoo/config/xiaoo.env
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/deleteWebhook" \
  -H 'Content-Type: application/json' \
  -d '{"drop_pending_updates": false}'
```

Verify that webhook mode is disabled:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Expected:

```json
{"ok":true,"result":{"url":""}}
```

The real response includes more fields, but `result.url` must be empty.

## 9. Webhook Mode Configuration

Webhook mode receives updates through HTTPS POST callbacks from Telegram.

Generate a webhook secret token:

```bash
openssl rand -base64 32 | tr -dc 'A-Za-z0-9_-' | head -c 32
```

Add these fields under `[channels.telegram]`:

```toml
[channels.telegram]
enabled = true
transport = "webhook"
channel_instance_id = "ops-telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"
webhook_secret_token = "replace-with-the-generated-secret"
bot_username = "@your_bot_username"
base_url = "https://api.telegram.org"
```

The xiaoO internal callback route is:

```text
POST /api/v1/channels/telegram/events
```

Telegram sends the configured webhook secret in this header:

```text
X-Telegram-Bot-Api-Secret-Token
```

xiaoO rejects the webhook request when the header does not match `channels.telegram.webhook_secret_token`.

## 10. Why Reverse Proxy Is Needed for Webhook

In the recommended server deployment, xiaoO does not bind directly on the public interface.

Instead:

- xiaoO listens on:
  - `127.0.0.1:18080`
- nginx listens publicly on:
  - `0.0.0.0:443`

This is recommended because:

- Telegram requires a public HTTPS webhook URL
- xiaoO itself stays on localhost
- TLS can be terminated at nginx
- multiple bots can be hosted behind different public paths

Polling mode does not need nginx for Telegram delivery, but the daemon can still expose health and local chat APIs on localhost.

## 11. Example nginx Routing

If the public Telegram webhook path is the same as the internal xiaoO path:

```nginx
location = /api/v1/channels/telegram/events {
    proxy_pass http://127.0.0.1:18080/api/v1/channels/telegram/events;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

If you want a public alias path, map it explicitly:

```nginx
location = /api/v1/channels/eulerclaw/events {
    proxy_pass http://127.0.0.1:18080/api/v1/channels/telegram/events;
    proxy_http_version 1.1;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
}
```

If you are creating nginx config from scratch:

```nginx
server {
    listen 443 ssl;
    server_name <your-domain>;

    ssl_certificate /etc/letsencrypt/live/<your-domain>/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/<your-domain>/privkey.pem;

    location = /api/v1/channels/telegram/events {
        proxy_pass http://127.0.0.1:18080/api/v1/channels/telegram/events;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    location = /api/v1/health {
        proxy_pass http://127.0.0.1:18080/api/v1/health;
    }
}
```

After editing nginx config:

```bash
nginx -t
systemctl reload nginx
```

## 12. Register the Telegram Webhook

After xiaoO and nginx are ready, register the webhook with Telegram:

```bash
source /opt/xiaoo/config/xiaoo.env
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/setWebhook" \
  -H 'Content-Type: application/json' \
  -d '{
    "url": "https://<your-domain>/api/v1/channels/telegram/events",
    "secret_token": "replace-with-the-generated-secret",
    "allowed_updates": ["message", "channel_post"]
  }'
```

Verify Telegram accepted it:

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Expected checks:

- `ok` is `true`
- `result.url` is the public webhook URL
- `result.pending_update_count` does not keep increasing after a test message
- `result.last_error_message` is absent or empty

## 13. systemd Service

If you are creating the service from scratch, use a full unit file.

```ini
[Unit]
Description=xiaoO Telegram daemon
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/xiaoo
EnvironmentFile=/opt/xiaoo/config/xiaoo.env
ExecStart=/opt/xiaoo/bin/xiaoo-app daemon --config /opt/xiaoo/config/config.toml --host 127.0.0.1 --port 18080
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

A few important details:

- `--host 127.0.0.1` means xiaoO is intentionally internal-only
- webhook mode uses nginx for public exposure
- polling mode does not need public exposure
- `EnvironmentFile` is where `TELEGRAM_BOT_TOKEN` and `OPENROUTER_API_KEY` are loaded from

After creating or editing the unit file:

```bash
systemctl daemon-reload
systemctl enable --now xiaoo-telegram.service
```

After changing either the config file or env file, restart the service:

```bash
systemctl restart xiaoo-telegram.service
```

If you changed the environment file, a restart is required.

## 14. Local macOS Polling Helper

For local development on macOS, a small wrapper script is usually enough.

Example environment file:

```bash
# ~/.config/xiaoo/telegram.env
TELEGRAM_BOT_TOKEN=your-real-telegram-bot-token
```

Example script:

```bash
#!/usr/bin/env zsh
set -euo pipefail

set -a
source "$HOME/.config/xiaoo/telegram.env"
set +a

export OPENROUTER_API_KEY="$(
python3 - <<'PY'
import json
import pathlib

path = pathlib.Path.home() / ".config" / "xiaoo" / "llm_secrets.json"
data = json.loads(path.read_text())
value = str(data.get("OPENROUTER_API_KEY", "")).strip()
if not value:
    raise SystemExit("OPENROUTER_API_KEY is missing from ~/.config/xiaoo/llm_secrets.json")
print(value)
PY
)"

cd "/path/to/xiaoO"
exec cargo run -p xiaoo-app --bin xiaoo-app -- daemon \
  --config "$HOME/.config/xiaoo/config.toml" \
  --host 127.0.0.1 \
  --port 18080
```

Recommended permissions:

```bash
chmod 600 ~/.config/xiaoo/telegram.env
chmod 700 ~/.config/xiaoo/run-telegram-polling.sh
```

## 15. Connection Establishment Checklists

### 15.1 Polling Checklist

These layers must line up for polling mode:

1. Telegram bot exists in BotFather
2. `TELEGRAM_BOT_TOKEN` is present in the daemon environment
3. `[channels.telegram].enabled = true`
4. `[channels.telegram].transport = "polling"`
5. `bot_token_env = "TELEGRAM_BOT_TOKEN"`
6. any existing webhook has been removed with `deleteWebhook`
7. `getWebhookInfo.result.url` is empty
8. xiaoO daemon is running
9. the daemon can reach `https://api.telegram.org`
10. the daemon can reach the model provider
11. the bot is added to the target chat or group
12. BotFather privacy mode matches the desired group behavior

### 15.2 Webhook Checklist

These layers must line up for webhook mode:

1. Telegram bot exists in BotFather
2. `TELEGRAM_BOT_TOKEN` is present in the daemon environment
3. `[channels.telegram].enabled = true`
4. `[channels.telegram].transport = "webhook"`
5. `webhook_secret_token` is configured
6. xiaoO daemon is listening on `127.0.0.1:18080`
7. nginx has a matching HTTPS `location`
8. nginx proxies to `/api/v1/channels/telegram/events`
9. public DNS resolves to your server
10. port `443` is reachable from the public internet
11. Telegram `setWebhook` uses the same public URL
12. Telegram `setWebhook.secret_token` matches `webhook_secret_token`
13. `getWebhookInfo.result.url` is the public callback URL
14. the daemon can reach the model provider and Telegram Bot API outbound

## 16. Manual Verification Commands

### 16.1 Check bot identity

```bash
source /opt/xiaoo/config/xiaoo.env
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getMe"
```

Expected:

```json
{"ok":true,"result":{"username":"your_bot_username"}}
```

### 16.2 Check daemon health

```bash
curl http://127.0.0.1:18080/api/v1/health
```

Expected:

```json
{"status":"ok","version":"0.1.0"}
```

### 16.3 Check polling readiness

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Expected for polling mode:

```json
{"ok":true,"result":{"url":""}}
```

### 16.4 Check webhook readiness

```bash
curl -sS "https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/getWebhookInfo"
```

Expected for webhook mode:

```json
{"ok":true,"result":{"url":"https://<your-domain>/api/v1/channels/telegram/events"}}
```

### 16.5 Check service logs

```bash
journalctl -u xiaoo-telegram.service -f
```

For local script-based polling:

```bash
tail -f ~/.config/xiaoo/logs/telegram-polling.log
```

### 16.6 Check nginx callback access

```bash
grep "api/v1/channels/telegram/events" /var/log/nginx/access.log | tail -n 20
```

This is useful when Telegram reports webhook errors but the application logs show nothing.

### 16.7 Confirm the daemon process is using the expected config

```bash
systemctl status xiaoo-telegram.service
```

Look for:

- `EnvironmentFile=/opt/xiaoo/config/xiaoo.env`
- `--config /opt/xiaoo/config/config.toml`
- `--host 127.0.0.1 --port 18080`

## 17. Message Handling Notes

xiaoO currently handles Telegram messages asynchronously:

- webhook requests return an acknowledgement before the agent work finishes
- polling updates are accepted from `getUpdates`, then processed in background tasks
- replies are sent through Telegram Bot API `sendMessage`

Supported update sources:

- `message`
- `channel_post`

Supported message shape:

- text messages
- direct chats
- group chats
- supergroup forum topics
- channel posts

Topic conversations are encoded as:

```text
chat_id:message_thread_id
```

This keeps different forum topics in separate xiaoO sessions.

When `bot_username` is configured, xiaoO strips leading bot invocations before sending text to the agent:

- `@your_bot hello` -> `hello`
- `/ask@your_bot hello` -> `hello`

Non-text updates are ignored.

## 18. Common Failure Modes

| Symptom | Likely Cause | What to Check |
|---|---|---|
| `getMe` fails | invalid or missing bot token | `TELEGRAM_BOT_TOKEN` in env file |
| polling logs say webhook is active | webhook was not deleted | `deleteWebhook`, then `getWebhookInfo` |
| webhook receives 401 | secret token mismatch | `webhook_secret_token` and `setWebhook.secret_token` |
| webhook never reaches xiaoO | public route problem | DNS, firewall, nginx access log |
| `getWebhookInfo` shows `last_error_message` | Telegram cannot deliver webhook | HTTPS certificate, nginx route, daemon health |
| bot works in DM but not group | privacy mode or group membership | BotFather `/setprivacy`, add bot to group |
| daemon receives message but no reply | model provider failure | `OPENROUTER_API_KEY`, outbound network, service logs |
| replies fail after processing | Telegram API send failure | bot still in chat, token not revoked, Bot API logs |
| local polling script starts then exits | missing env var or wrong binary | script output, `TELEGRAM_BOT_TOKEN`, `OPENROUTER_API_KEY` |
| callback endpoint says not configured | wrong transport or route | use webhook mode for HTTP callback, polling mode has no callback |

## 19. Recommended Production Layout

For a clean webhook production deployment, use this structure:

```text
/opt/xiaoo/bin/xiaoo-app
/opt/xiaoo/config/config.toml
/opt/xiaoo/config/xiaoo.env
/opt/xiaoo/app
/etc/systemd/system/xiaoo-telegram.service
/etc/nginx/conf.d/xiaoo-telegram.conf
```

And keep the responsibility split like this:

- Telegram platform:
  - message source
- nginx:
  - public HTTPS ingress for webhook mode
- xiaoO daemon:
  - polling loop or webhook handling and runtime execution
- Telegram Bot API:
  - outbound `getUpdates`, `setWebhook`, `sendMessage`

For a clean local polling deployment, use this structure:

```text
~/.config/xiaoo/config.toml
~/.config/xiaoo/telegram.env
~/.config/xiaoo/llm_secrets.json
~/.config/xiaoo/run-telegram-polling.sh
~/.config/xiaoo/logs/telegram-polling.log
```

## 20. Features Currently Available in Telegram

| Feature | Status | Notes |
|---|---|---|
| Text messages | Supported | direct, group, supergroup, channel post |
| Reply by bot | Supported | sent via Telegram Bot API `sendMessage` |
| Webhook delivery | Supported | `transport = "webhook"` |
| Local long polling | Supported | `transport = "polling"` using `getUpdates` |
| WebSocket delivery | Not supported by Telegram Bot API | use polling for local long connection behavior |
| Forum topics | Supported | session key includes `message_thread_id` |
| Bot mention stripping | Supported | requires `bot_username` |
| Reactions | Not implemented | Telegram adapter currently sends text replies |
| Media attachments | Not implemented | non-text updates are ignored |
| Group member directory | Not supported | Telegram adapter does not list members |

## 21. Final Deployment Checklist

Before handing the deployment to someone else, make sure they can answer **yes** to all relevant items.

For both modes:

- Do you have the correct bot token from BotFather?
- Is the token stored only in the env file?
- Can the daemon read `TELEGRAM_BOT_TOKEN` and `OPENROUTER_API_KEY`?
- Does `getMe` return the expected bot username?
- Does `curl http://127.0.0.1:18080/api/v1/health` return ok?
- Can the daemon reach both Telegram Bot API and the model provider?
- Is the bot added to the target chat or group?
- Is BotFather privacy mode configured for the expected group behavior?

For polling mode:

- Is `transport = "polling"` set?
- Has any existing webhook been deleted?
- Does `getWebhookInfo.result.url` return an empty string?
- Is the polling daemon process still running?

For webhook mode:

- Is `transport = "webhook"` set?
- Is `webhook_secret_token` configured?
- Is nginx routing the public webhook path to `/api/v1/channels/telegram/events`?
- Did you run `nginx -t` and reload nginx?
- Does `getWebhookInfo.result.url` match the public HTTPS URL?
- Does nginx access log show Telegram webhook requests?

If all relevant checks pass, the Telegram deployment should be reproducible.
