use std::io::Write;

use futures_util::StreamExt;
use serde_json::Value;
use xiaoo_shared::gateway::{
    AppTurnRequest, GatewayEntryContext, SessionDetachRequest, SessionOpenRequest,
};

use super::entry::OutputFormat;

pub(super) async fn run_with_attach(
    url: &str,
    prompt: String,
    format: OutputFormat,
    title: Option<String>,
    session: Option<String>,
    agent: Option<String>,
    debug: bool,
) {
    let base_url = url.trim_end_matches('/').to_string();
    if debug {
        eprintln!("Attaching to daemon at: {base_url}");
    }

    // Optional bearer token so attach also works against daemons that enable
    // HTTP bearer auth (mirrors the TUI's resolve_bearer_token).
    let bearer_token = std::env::var("XIAOO_DAEMON_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // This process's identity for the daemon's attach-lease table.
    let client_id = format!("cli-{}", std::process::id());
    let session_id = session.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let is_json = format == OutputFormat::Json;

    if is_json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "type": "session_start",
                "data": {
                    "session_id": &session_id,
                    "title": &title,
                    "agent": &agent,
                }
            }))
            .unwrap()
        );
        let _ = std::io::stdout().flush();
    }

    // 1. Open (or re-open) the session so this process holds the lease --
    //    required before submitting a turn when the daemon enforces lease
    //    ownership (`XIAOO_ENFORCE_LEASE=on`).
    let open_request = SessionOpenRequest {
        session_id: session_id.clone(),
        conversation_id: session_id.clone(),
        sender_id: "cli-user".to_string(),
        entry: GatewayEntryContext::cli(),
        channel: None,
        channel_instance_id: None,
        llm: None,
        workspace: None,
        skills: None,
        client_id: Some(client_id.clone()),
        client_pid: Some(std::process::id()),
        client_hostname: None,
    };
    let open_url = format!("{base_url}/api/v1/runtimes/open");
    let mut open_req = client.post(&open_url).json(&open_request);
    if let Some(token) = bearer_token.as_ref() {
        open_req = open_req.bearer_auth(token);
    }
    match open_req.send().await {
        Ok(resp) if !resp.status().is_success() => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            attach_fail(
                format!("session open failed: HTTP {status} {body}"),
                is_json,
            );
        }
        Ok(_) => {}
        Err(error) => attach_fail(format!("failed to connect to daemon: {error}"), is_json),
    }

    // 2. Submit the turn; the daemon replies with an SSE event stream.
    let turn_request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext::cli(),
        channel: None,
        message_id: None,
        conversation_id: session_id.clone(),
        sender_id: "cli-user".to_string(),
        text: prompt,
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
        reasoning_effort: None,
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        client_id: Some(client_id.clone()),
    };
    let input_url = format!("{base_url}/api/v1/runtimes/input");
    let mut input_req = client.post(&input_url).json(&turn_request);
    if let Some(token) = bearer_token.as_ref() {
        input_req = input_req.bearer_auth(token);
    }
    let response = match input_req.send().await {
        Ok(response) => response,
        Err(error) => attach_fail(format!("failed to submit turn: {error}"), is_json),
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        attach_fail(
            format!("turn submission failed: HTTP {status} {body}"),
            is_json,
        );
    }

    // 3. Consume the SSE event stream emitted by /api/v1/runtimes/input.
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut printed_any_text = false;
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => attach_fail(format!("stream read error: {error}"), is_json),
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(frame) = take_sse_frame(&mut buffer) {
            let Some(event) = parse_sse_event(&frame) else {
                continue;
            };
            let typ = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match typ {
                "text_delta" => {
                    if is_json {
                        println!("{}", serde_json::to_string(&event).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    } else if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                        print!("{delta}");
                        let _ = std::io::stdout().flush();
                        printed_any_text = true;
                    }
                }
                "done" => {
                    saw_done = true;
                    if is_json {
                        println!("{}", serde_json::to_string(&event).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    } else if !printed_any_text {
                        // Daemon sent no incremental deltas; emit the final reply.
                        if let Some(reply) = event.get("reply").and_then(|v| v.as_str()) {
                            println!("{reply}");
                            let _ = std::io::stdout().flush();
                        }
                    }
                }
                "error" => {
                    let message = event
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown daemon error");
                    attach_fail(message.to_string(), is_json);
                }
                _ => {
                    if is_json {
                        println!("{}", serde_json::to_string(&event).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    }
                }
            }
        }
    }

    if !saw_done {
        attach_fail(
            "daemon stream ended without a completion event".to_string(),
            is_json,
        );
    }

    // 4. Best-effort detach so the daemon releases this process's lease
    //    promptly instead of waiting for the staleness timeout. Errors are
    //    ignored -- the turn already completed successfully.
    let detach_request = SessionDetachRequest {
        session_id: session_id.clone(),
        client_id: Some(client_id),
    };
    let detach_url = format!("{base_url}/api/v1/runtimes/detach");
    let mut detach_req = client.post(&detach_url).json(&detach_request);
    if let Some(token) = bearer_token.as_ref() {
        detach_req = detach_req.bearer_auth(token);
    }
    let _ = detach_req.send().await;
}

/// Report an attach-mode failure to stderr (text) or stdout (JSON), then exit.
fn attach_fail(message: String, is_json: bool) -> ! {
    if is_json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "type": "error",
                "data": { "message": message }
            }))
            .unwrap()
        );
        let _ = std::io::stdout().flush();
    } else {
        eprintln!("{message}");
    }
    std::process::exit(1);
}

/// Extract a single SSE frame (text up to a blank line) from the buffer.
fn take_sse_frame(buffer: &mut String) -> Option<String> {
    let index = buffer.find("\n\n")?;
    let frame = buffer[..index].to_string();
    buffer.drain(..index + 2);
    Some(frame)
}

/// Parse an SSE frame's `data:` lines into a single JSON value. Returns
/// `None` for keep-alive/comment frames or malformed JSON.
fn parse_sse_event(frame: &str) -> Option<Value> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    serde_json::from_str(&data).ok()
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/cli/attach_test.rs"]
mod attach_sse_tests;
