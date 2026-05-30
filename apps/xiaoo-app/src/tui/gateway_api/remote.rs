use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use agent_types::common::ids::AgentId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};

use crate::app_state::AppState;
use crate::chat::{Message, ToolExecutionStatus, ToolExecutionUpdate};
use crate::gateway::{AppTurnRequest, GatewayEntryContext, SessionOpenRequest};
use crate::interaction_prompt::{PromptChoice, PromptRequest, PromptResolution, UserPromptResult};
use crate::session_gateway::SessionTurnUpdate;

use super::runtime::GatewayRuntime;

#[derive(Clone, Debug)]
pub struct RemoteRuntimeConfig {
    pub base_url: String,
    pub bearer_token_env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RemoteSseEvent {
    TurnStart {
        #[allow(dead_code)]
        agent_id: String,
        #[allow(dead_code)]
        turn: u32,
    },
    TextDelta {
        #[allow(dead_code)]
        delta: String,
        snapshot: String,
    },
    ThinkingDelta {
        #[allow(dead_code)]
        delta: String,
        snapshot: String,
    },
    ToolResult {
        call_id: String,
        tool_name: String,
        output_preview: String,
        is_error: bool,
    },
    InteractionRequested {
        request: InteractionRequest,
    },
    Done {
        #[allow(dead_code)]
        reply: String,
        #[allow(dead_code)]
        raw_reply: String,
        #[allow(dead_code)]
        conversation_id: String,
        #[allow(dead_code)]
        session_id: String,
        #[allow(dead_code)]
        turn_count: u32,
        total_tokens: usize,
        prompt_tokens: u64,
        completion_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<llm_client::ChatMessage>,
        #[allow(dead_code)]
        stop_reason: String,
    },
    Error {
        error: String,
    },
    Cancelled {
        session_id: String,
    },
}

impl GatewayRuntime {
    pub fn configure_remote(
        &mut self,
        state: &mut AppState,
        base_url: String,
        bearer_token_env: Option<String>,
    ) {
        let base_url = normalize_base_url(&base_url);
        self.remote = Some(RemoteRuntimeConfig {
            base_url: base_url.clone(),
            bearer_token_env,
        });
        self.remote_session_open = false;
        state
            .status_panel
            .set_backend(format!("Remote: {base_url}"));
        state.status_panel.set_remote_workspace(&base_url);
    }

    pub async fn connect_remote(
        &mut self,
        state: &mut AppState,
        base_url: String,
        bearer_token_env: Option<String>,
    ) -> Result<String, String> {
        let base_url = normalize_base_url(&base_url);
        let token = resolve_bearer_token(bearer_token_env.as_deref())?;
        let client = reqwest::Client::new();
        let mut request = client.get(format!("{base_url}/api/v1/health"));
        if let Some(token) = token.as_ref() {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("remote health check failed: {error}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "remote health check failed: HTTP {}",
                response.status()
            ));
        }
        self.configure_remote(state, base_url.clone(), bearer_token_env);
        Ok(format!("Remote connected: {base_url}"))
    }

    pub async fn disconnect_remote(&mut self, state: &mut AppState) -> Result<(), String> {
        self.close_remote_session(&state.session_id).await;
        self.remote = None;
        self.remote_session_open = false;
        state.status_panel.set_backend("Local");
        state.status_panel.set_workspace(&state.workspace);
        Ok(())
    }

    pub async fn remote_status(&self) -> String {
        let Some(remote) = self.remote.as_ref() else {
            return "Backend: Local".to_string();
        };

        let token = match resolve_bearer_token(remote.bearer_token_env.as_deref()) {
            Ok(token) => token,
            Err(error) => return format!("Backend: Remote {}\nHealth: {error}", remote.base_url),
        };
        let client = reqwest::Client::new();
        let mut request = client.get(format!("{}/api/v1/health", remote.base_url));
        if let Some(token) = token.as_ref() {
            request = request.bearer_auth(token);
        }
        let health = match request.send().await {
            Ok(response) if response.status().is_success() => "ok".to_string(),
            Ok(response) => format!("HTTP {}", response.status()),
            Err(error) => error.to_string(),
        };
        format!(
            "Backend: Remote {}\nSession open: {}\nHealth: {}",
            remote.base_url, self.remote_session_open, health
        )
    }

    pub fn remote_base_url(&self) -> Option<&str> {
        self.remote.as_ref().map(|remote| remote.base_url.as_str())
    }

    pub(super) async fn start_remote_turn(
        &mut self,
        state: &mut AppState,
        prompt: String,
        append_user_message: bool,
    ) -> Result<(), String> {
        let remote = self
            .remote
            .clone()
            .ok_or_else(|| "remote backend is not configured".to_string())?;
        let token = resolve_bearer_token(remote.bearer_token_env.as_deref())?;
        let client = reqwest::Client::new();

        if !self.remote_session_open {
            let open_request = self.remote_session_open_request(state)?;
            post_json(
                &client,
                &remote,
                token.as_deref(),
                "/api/v1/sessions/open",
                &open_request,
            )
            .await?;
            self.remote_session_open = true;
        }

        let turn_request = self.remote_turn_request(state, prompt.clone())?;

        state.chat_state.stick_to_bottom = true;
        self.request_start = Some(std::time::Instant::now());
        self.first_token_latency_recorded = false;
        if append_user_message {
            state.chat_state.messages.push(Message::user(prompt));
            state.chat_state.input.reset();
        }
        state.chat_state.is_loading = true;
        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        self.stream_message_index = Some(state.chat_state.messages.len().saturating_sub(1));
        self.stream_reveal_buffer.clear();
        self.pending_stream_done = None;

        let (updates_tx, updates_rx) = unbounded_channel();
        let (interaction_tx, interaction_rx) = unbounded_channel();
        self.interaction_reply_tx = Some(interaction_tx);
        self.stream_rx = Some(updates_rx);

        tokio::spawn(async move {
            run_remote_stream(
                client,
                remote,
                token,
                turn_request,
                updates_tx,
                interaction_rx,
            )
            .await;
        });

        Ok(())
    }

    pub async fn close_remote_session(&mut self, session_id: &str) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let Ok(token) = resolve_bearer_token(remote.bearer_token_env.as_deref()) else {
            return;
        };
        let client = reqwest::Client::new();
        let path = format!("/api/v1/sessions/{session_id}/close");
        let _ = post_empty(&client, &remote, token.as_deref(), &path).await;
        self.remote_session_open = false;
    }

    pub fn cancel_remote_turn(&self, session_id: String) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let Ok(token) = resolve_bearer_token(remote.bearer_token_env.as_deref()) else {
            return;
        };
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let path = format!("/api/v1/sessions/{session_id}/cancel");
            let _ = post_empty(&client, &remote, token.as_deref(), &path).await;
        });
    }

    fn remote_session_open_request(&self, state: &AppState) -> Result<SessionOpenRequest, String> {
        let sender_id = super::runtime_request::resolve_agent_id(None, None, &state.agent_config)?;
        Ok(SessionOpenRequest {
            session_id: state.session_id.clone(),
            conversation_id: state.session_id.clone(),
            sender_id,
            entry: GatewayEntryContext::tui(self.remote.as_ref().map(|r| r.base_url.clone())),
            channel: None,
            channel_instance_id: None,
        })
    }

    fn remote_turn_request(
        &self,
        state: &AppState,
        text: String,
    ) -> Result<AppTurnRequest, String> {
        let sender_id = super::runtime_request::resolve_agent_id(None, None, &state.agent_config)?;
        Ok(AppTurnRequest {
            session_id: state.session_id.clone(),
            entry: GatewayEntryContext::tui(self.remote.as_ref().map(|r| r.base_url.clone())),
            channel: None,
            message_id: None,
            conversation_id: state.session_id.clone(),
            sender_id,
            text,
            channel_instance_id: None,
            channel_identity_prompt: None,
            reply_to_message_id: None,
            root_message_id: None,
            mentions: Vec::new(),
            reasoning_effort: state.reasoning_effort,
        })
    }
}

async fn run_remote_stream(
    client: reqwest::Client,
    remote: RemoteRuntimeConfig,
    token: Option<String>,
    turn_request: AppTurnRequest,
    updates_tx: UnboundedSender<SessionTurnUpdate>,
    mut interaction_rx: UnboundedReceiver<UserPromptResult>,
) {
    let url = format!(
        "{}/api/v1/sessions/{}/turn/stream",
        remote.base_url, turn_request.session_id
    );
    let mut request = client.post(url).json(&turn_request);
    if let Some(token) = token.as_ref() {
        request = request.bearer_auth(token);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            let _ = updates_tx.send(SessionTurnUpdate::Err(error.to_string()));
            return;
        }
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let _ = updates_tx.send(SessionTurnUpdate::Err(format!(
            "remote turn failed: HTTP {status} {body}"
        )));
        return;
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = updates_tx.send(SessionTurnUpdate::Err(error.to_string()));
                return;
            }
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(frame) = take_sse_frame(&mut buffer) {
            match parse_sse_frame(&frame) {
                Ok(Some(event)) => {
                    handle_remote_event(
                        event,
                        &client,
                        &remote,
                        token.as_deref(),
                        &turn_request.session_id,
                        &updates_tx,
                        &mut interaction_rx,
                    )
                    .await;
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = updates_tx.send(SessionTurnUpdate::Err(error));
                    return;
                }
            }
        }
    }
}

async fn handle_remote_event(
    event: RemoteSseEvent,
    client: &reqwest::Client,
    remote: &RemoteRuntimeConfig,
    token: Option<&str>,
    session_id: &str,
    updates_tx: &UnboundedSender<SessionTurnUpdate>,
    interaction_rx: &mut UnboundedReceiver<UserPromptResult>,
) {
    match event {
        RemoteSseEvent::TurnStart { .. } => {}
        RemoteSseEvent::TextDelta { snapshot, .. } => {
            let _ = updates_tx.send(SessionTurnUpdate::SetAssistantContent {
                agent_id: AgentId("cli-agent".to_string()),
                text: snapshot,
            });
        }
        RemoteSseEvent::ThinkingDelta { snapshot, .. } => {
            let _ = updates_tx.send(SessionTurnUpdate::SetAssistantThinking {
                agent_id: AgentId("cli-agent".to_string()),
                text: snapshot,
            });
        }
        RemoteSseEvent::ToolResult {
            call_id,
            tool_name,
            output_preview,
            is_error,
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::Tool {
                _agent_id: AgentId("cli-agent".to_string()),
                update: ToolExecutionUpdate {
                    call_id,
                    tool: tool_name,
                    summary: if is_error {
                        "remote tool failed".to_string()
                    } else {
                        "remote tool completed".to_string()
                    },
                    args_preview: String::new(),
                    command_preview: None,
                    command: None,
                    detail: output_preview,
                    status: if is_error {
                        ToolExecutionStatus::Failed
                    } else {
                        ToolExecutionStatus::Completed
                    },
                    exit_code: None,
                    duration_ms: None,
                    file_change: None,
                },
            });
        }
        RemoteSseEvent::InteractionRequested { request } => {
            let prompt = build_prompt_request(&request);
            let _ = updates_tx.send(SessionTurnUpdate::InteractionPrompt(prompt.clone()));
            while let Some(result) = interaction_rx.recv().await {
                if result.request_id != prompt.request_id {
                    continue;
                }
                let response = map_response(&request, result)
                    .unwrap_or_else(|| default_interaction_response(&request));
                let path = format!("/api/v1/sessions/{session_id}/interaction");
                let _ = post_json(client, remote, token, &path, &response).await;
                break;
            }
        }
        RemoteSseEvent::Done {
            total_tokens,
            prompt_tokens,
            completion_tokens,
            estimated_input_tokens,
            messages,
            ..
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::Done {
                prompt_tokens,
                completion_tokens,
                total_tokens: total_tokens as u64,
                estimated_input_tokens,
                messages,
            });
        }
        RemoteSseEvent::Error { error } => {
            let _ = updates_tx.send(SessionTurnUpdate::Err(error));
        }
        RemoteSseEvent::Cancelled { session_id } => {
            let _ = updates_tx.send(SessionTurnUpdate::Err(format!(
                "remote turn cancelled for session {session_id}"
            )));
        }
    }
}

fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn resolve_bearer_token(env_name: Option<&str>) -> Result<Option<String>, String> {
    let Some(env_name) = env_name.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let value = std::env::var(env_name)
        .map_err(|_| format!("remote bearer token env var {env_name} is not set"))?;
    if value.trim().is_empty() {
        return Err(format!("remote bearer token env var {env_name} is empty"));
    }
    Ok(Some(value))
}

async fn post_empty(
    client: &reqwest::Client,
    remote: &RemoteRuntimeConfig,
    token: Option<&str>,
    path: &str,
) -> Result<(), String> {
    let mut request = client.post(format!("{}{}", remote.base_url, path));
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("HTTP {}", response.status()))
    }
}

async fn post_json<T: Serialize + ?Sized>(
    client: &reqwest::Client,
    remote: &RemoteRuntimeConfig,
    token: Option<&str>,
    path: &str,
    body: &T,
) -> Result<(), String> {
    let mut request = client
        .post(format!("{}{}", remote.base_url, path))
        .json(body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(format!("HTTP {status} {body}"))
    }
}

fn take_sse_frame(buffer: &mut String) -> Option<String> {
    let index = buffer.find("\n\n")?;
    let frame = buffer[..index].to_string();
    buffer.drain(..index + 2);
    Some(frame)
}

fn parse_sse_frame(frame: &str) -> Result<Option<RemoteSseEvent>, String> {
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
        return Ok(None);
    }
    let data = data_lines.join("\n");
    serde_json::from_str(&data)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn build_prompt_request(request: &InteractionRequest) -> PromptRequest {
    match request {
        InteractionRequest::Confirm { prompt, .. } => PromptRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            title: prompt.clone(),
            body: None,
            choices: vec![
                PromptChoice {
                    id: "approve".to_string(),
                    label: "Approve".to_string(),
                    description: None,
                },
                PromptChoice {
                    id: "reject".to_string(),
                    label: "Reject".to_string(),
                    description: None,
                },
            ],
            allow_custom_input: false,
            multi_select: false,
            default_index: Some(0),
        },
        InteractionRequest::TextInput { prompt, .. } => PromptRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            title: prompt.clone(),
            body: None,
            choices: vec![PromptChoice {
                id: "submit".to_string(),
                label: "Submit".to_string(),
                description: None,
            }],
            allow_custom_input: true,
            multi_select: false,
            default_index: Some(0),
        },
        InteractionRequest::Choice {
            prompt,
            options,
            allow_custom_input,
            ..
        } => PromptRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            title: prompt.clone(),
            body: None,
            choices: options
                .iter()
                .map(|option| PromptChoice {
                    id: option.clone(),
                    label: option.clone(),
                    description: None,
                })
                .collect(),
            allow_custom_input: *allow_custom_input,
            multi_select: false,
            default_index: Some(0),
        },
    }
}

fn map_response(
    request: &InteractionRequest,
    response: UserPromptResult,
) -> Option<InteractionResponse> {
    match (request, response.resolution) {
        (InteractionRequest::Confirm { .. }, PromptResolution::Single { choice_id, .. }) => {
            Some(InteractionResponse::Confirmed {
                allowed: choice_id == "approve",
            })
        }
        (InteractionRequest::TextInput { .. }, PromptResolution::Single { supplement, .. }) => {
            Some(InteractionResponse::Text { value: supplement })
        }
        (
            InteractionRequest::Choice { .. },
            PromptResolution::Single {
                choice_id,
                supplement,
            },
        ) => Some(InteractionResponse::Choice {
            value: supplement.or(Some(choice_id)),
        }),
        (_, PromptResolution::Cancelled) => None,
        (_, PromptResolution::Multi { .. }) => None,
    }
}

fn default_interaction_response(request: &InteractionRequest) -> InteractionResponse {
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
        InteractionRequest::TextInput { .. } => InteractionResponse::Text { value: None },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice { value: None },
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_sse_frame, take_sse_frame, RemoteSseEvent};

    #[test]
    fn parses_sse_frame_from_split_buffer() {
        let mut buffer = String::from(
            "event: text_delta\ndata: {\"type\":\"text_delta\",\"delta\":\"he\",\"snapshot\":\"he\"}\n\nrest",
        );
        let frame = take_sse_frame(&mut buffer).expect("frame");
        let parsed = parse_sse_frame(&frame).expect("parse").expect("event");
        match parsed {
            RemoteSseEvent::TextDelta { delta, snapshot } => {
                assert_eq!(delta, "he");
                assert_eq!(snapshot, "he");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(buffer, "rest");
    }

    #[test]
    fn ignores_keepalive_frame() {
        let parsed = parse_sse_frame(": keepalive").expect("parse");
        assert!(parsed.is_none());
    }
}
