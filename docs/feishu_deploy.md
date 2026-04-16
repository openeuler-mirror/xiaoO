# Feishu Channel Deployment Guide

This guide covers how to connect xiaoO to Feishu so that users can interact with xiaoO through Feishu messages.

## Overview

xiaoO receives Feishu messages via **Webhook** (HTTP POST). The flow is:

```
User sends message in Feishu
  -> Feishu platform POSTs event to xiaoO webhook endpoint
  -> xiaoO processes the message and replies via Feishu API
```

## Prerequisites

- A Feishu tenant (organization) account with admin access
- xiaoO daemon running and accessible from the internet (or via reverse proxy)
- A public URL that Feishu can reach (e.g., `https://your-domain.com`)

---

## Step 1: Create Feishu Application

1. Go to [Feishu Open Platform](https://open.feishu.cn/) and log in
2. Click **Create App** -> **Custom App**
3. Fill in app name and description
4. In the left sidebar, go to **App Capabilities** -> **Add Capability** -> select **Bot**

## Step 2: Collect Credentials

In the app detail page, go to **Credentials & Basic Info**. Record these values:

| Field | Where to Find | Used For |
|-------|--------------|----------|
| **App ID** | Credentials & Basic Info | `config.toml` `app_id` |
| **App Secret** | Credentials & Basic Info | Environment variable (e.g., `FEISHU_APP_SECRET`) |

> **Security**: Never commit App Secret to version control. Store it as an environment variable.

## Step 3: Configure Permissions

In the left sidebar, go to **Permissions & Scopes** and add:

| Permission | Scope ID | Purpose |
|-----------|----------|---------|
| Send messages as bot | `im:message:send_as_bot` | Send replies to users |
| Read private messages | `im:message.p2p_msg:readonly` | Receive DMs |
| Read group messages | `im:message.group_msg:readonly` | Receive group messages |
| Get user basic info | `contact:user.base:readonly` | Identify message senders |

## Step 4: Configure Event Subscription

1. In the left sidebar, go to **Events & Callbacks**
2. **Subscription Method**: Select **Webhook** (not long connection)
3. **Request URL**: Enter your xiaoO webhook endpoint:
   ```
   https://your-domain.com/api/v1/channels/feishu/events
   ```
   > xiaoO listens on `POST /api/v1/channels/feishu/events` for Feishu events.
   > If xiaoO is behind a reverse proxy (e.g., nginx), make sure this path is forwarded correctly.
4. Feishu will send a **URL verification** challenge. xiaoO handles this automatically — make sure the daemon is running before you click verify.
5. After verification succeeds, record the **Verification Token** from this page.
6. **Add Event**: Search and add `im.message.receive_v1` (Receive messages). Make sure the event toggle is enabled.

## Step 5: Publish the App

1. In the left sidebar, go to **App Release** -> **Version Management**
2. Create a new version and submit for review
3. The tenant admin approves and publishes the app in the Feishu Admin Console

> After publishing, add the bot to a group chat or start a DM with it to test.

---

## Step 6: Configure xiaoO

### config.toml

Add the Feishu channel section to your xiaoO config file:

```toml
[channels.feishu]
enabled = true
app_id = "cli_xxxxxxxxxxxx"               # App ID from Step 2
app_secret_env = "FEISHU_APP_SECRET"       # Name of the env var holding App Secret
verification_token = "xxxxxxxxxxxxxxxx"    # Verification Token from Step 4
base_url = "https://open.feishu.cn"        # Optional, this is the default

[channels]
interaction_timeout_secs = 600             # Optional, timeout for ask_user_question (default: 600s)
```

**Field reference:**

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `enabled` | No | `false` | Enable Feishu channel |
| `app_id` | Yes | — | Feishu App ID |
| `app_secret_env` | Yes | — | Name of the environment variable containing the App Secret |
| `verification_token` | Yes | — | Verification Token from event subscription page |
| `base_url` | No | `https://open.feishu.cn` | Feishu API base URL. Use `https://open.larksuite.com` for Lark (international) |
| `interaction_timeout_secs` | No | `600` | Timeout in seconds for interactive questions. Rounded up to nearest minute. |

### Environment Variable

Set the App Secret as an environment variable:

```bash
export FEISHU_APP_SECRET="your-app-secret-here"
```

For systemd service, add it to the `EnvironmentFile`:

```bash
# /opt/xiaoo/config/xiaoo.env
FEISHU_APP_SECRET=your-app-secret-here
```

### Start the Daemon

```bash
xiaoo-app daemon --config /path/to/config.toml --host 0.0.0.0 --port 8080
```

Or via systemd:

```ini
[Service]
EnvironmentFile=/opt/xiaoo/config/xiaoo.env
ExecStart=/opt/xiaoo/bin/xiaoo-app daemon --config /opt/xiaoo/config/config.toml --host 0.0.0.0 --port 8080
```

---

## Network Setup

xiaoO needs to be reachable from Feishu's servers. Common setups:

### Option A: Direct Public Access

If the server has a public IP:

```
Feishu -> http://your-public-ip:8080/api/v1/channels/feishu/events
```

### Option B: Reverse Proxy (Recommended)

Use nginx to add HTTPS and forward traffic:

```nginx
server {
    listen 443 ssl;
    server_name your-domain.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location /api/v1/channels/feishu/events {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_read_timeout 60s;
    }
}
```

### Option C: Tunnel (Development)

For local development, use a tunnel service:

```bash
# Using localhost.run
ssh -R 80:localhost:8080 nokey@localhost.run

# Using ngrok
ngrok http 8080
```

Use the tunnel URL as the webhook endpoint in Feishu.

---

## Verification

### 1. Health Check

```bash
curl http://localhost:8080/api/v1/health
# Expected: {"status":"ok","version":"0.1.0"}
```

### 2. Check Logs

```bash
journalctl -u xiaoo-rebuild.service -f
# Look for: "starting rebuild daemon ... addr=..."
```

### 3. Test in Feishu

- Add the bot to a group chat or send a DM
- Send a message like "hello"
- xiaoO should reply within a few seconds

### 4. Common Issues

| Symptom | Cause | Fix |
|---------|-------|-----|
| No reply from bot | Webhook URL not reachable | Check network/proxy, verify URL in Feishu console |
| `503 feishu webhook is not configured` | `[channels.feishu]` not in config or `enabled = false` | Check config.toml |
| `Authentication` error in logs | `verification_token` mismatch | Copy the exact token from Feishu event subscription page |
| `tenant access token request failed` | Wrong App Secret | Check `FEISHU_APP_SECRET` env var |
| Bot doesn't respond in groups | Missing group message permission | Add `im:message.group_msg:readonly` permission |

---

## Features Available in Feishu Channel

| Feature | Status | Description |
|---------|--------|-------------|
| Text messages | Supported | Send and receive text messages |
| Interactive questions | Supported | `ask_user_question` sends questions, user replies in chat |
| File sending | Supported | `send_file` tool uploads and sends files to the conversation |
| Progress updates | Supported | Shows processing progress as interactive cards |
| Reactions | Supported | Acknowledges received messages with emoji |
| Member listing | Supported | Resolves group member identities |
| Group chats | Supported | Works in group chats (mention bot to trigger) |
| Direct messages | Supported | Works in 1-on-1 DMs |
