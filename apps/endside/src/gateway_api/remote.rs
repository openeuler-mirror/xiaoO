use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use agent_types::common::ids::AgentId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use xiaoo_shared::plan::{SpawnSubagentMetadata, TodoSnapshotItem, TodoSnapshotUpdate};

use crate::app_state::{sandbox_display_name, AppState};
use crate::chat::{Message, ToolExecutionStatus, ToolExecutionUpdate};
use crate::gateway::{
    RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, RuntimeTurnRequest,
};
use crate::interaction_prompt::{PromptChoice, PromptRequest, PromptResolution, UserPromptResult};
use crate::remote_sessions_service::record_remote_session;
use crate::session_gateway::SessionTurnUpdate;

// TUI-side HTTP timeouts live in `crate::gateway_api::http_timeouts` so the
// shared crate does not carry TUI-only configuration.
use crate::gateway_api::http_timeouts::{
    EXIT_RPC_TIMEOUT, HEARTBEAT_RPC_TIMEOUT, OPEN_RPC_TIMEOUT, POST_JSON_SAFETY_TIMEOUT,
};

use super::runtime::GatewayRuntime;

/// Outcomes of a periodic `/runtimes/heartbeat` call from the TUI.
#[derive(Debug)]
pub enum HeartbeatError {
    /// Another TUI holds the lease. When `stale == Some(true)` the recorded
    /// holder is itself presumed dead and the caller may reclaim via
    /// `open_remote_session_with_record` instead of flagging a takeover.
    /// `stale` is `None` on parse failure (treated as `Some(false)` so a
    /// parse regression never widens the reclaim bypass).
    TakenOver { detail: String, stale: Option<bool> },
    /// Transient transport failure (network down, daemon restart, 5xx). The
    /// App ignores these — the next tick retries. A long outage eventually
    /// resolves as `TakenOver` (lease expired + acquired) or recovery.
    Network(String),
}

#[derive(Clone, Debug)]
pub struct RemoteRuntimeConfig {
    pub base_url: String,
    pub bearer_token_env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RemoteSseEvent {
    TurnStart {
        agent_id: String,
        turn: u32,
    },
    TextDelta {
        #[serde(default)]
        agent_id: Option<String>,
        #[allow(dead_code)]
        delta: String,
        snapshot: String,
    },
    ThinkingDelta {
        #[serde(default)]
        agent_id: Option<String>,
        #[allow(dead_code)]
        delta: String,
        snapshot: String,
    },
    ToolResult {
        #[serde(default)]
        agent_id: Option<String>,
        call_id: String,
        tool_name: String,
        output_preview: String,
        is_error: bool,
    },
    /// Per-call file change delta precomputed by the daemon. The TUI applies
    /// it directly to its session-diff tracker via `apply_remote_delta`,
    /// mirroring what the local-mode computation would have produced.
    ToolFileChange {
        call_id: String,
        file_path: String,
        additions: u32,
        deletions: u32,
    },
    /// Plan snapshot precomputed by the daemon from the `todo_write` tool's
    /// args. The TUI applies it directly to `state.plan_state`, mirroring
    /// what the local-mode `todo_snapshot_from_tool_args` would produce.
    PlanUpdate {
        title: String,
        items: Vec<TodoSnapshotItem>,
    },
    /// Subagent lane metadata precomputed by the daemon from the
    /// `spawn_subagent` tool's args + output. The TUI creates/updates the
    /// subagent lane directly, mirroring what the local-mode
    /// `parse_spawn_subagent_metadata_from_args` would produce.
    SubagentSpawn {
        agent_id: String,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
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
        #[serde(rename = "runtime_id", alias = "session_id")]
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
        #[serde(default)]
        actions: Vec<agent_types::hook::HookAction>,
    },
    Error {
        error: String,
    },
    Cancelled {
        #[serde(rename = "runtime_id", alias = "session_id")]
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
        // New session gets a fresh attach lease; clear any stale takeover
        // flag so the first submission after `/remote <url>` is not rejected.
        state.session_taken_over = false;
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
        let client = self.http_client.clone();
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
        // Detach this TUI's lease before clearing local state, so the daemon
        // doesn't keep the session "held by us" for the 45 s staleness window
        // (which would block another TUI's `/sessions` pickup meanwhile).
        if self.remote.is_some() && self.remote_session_open {
            let session_id = state.session_id.clone();
            self.detach_remote_session_bounded(&session_id, "disconnect_remote")
                .await;
        }
        self.remote = None;
        self.remote_session_open = false;
        // Takeover flag belongs to the abandoned remote session; clear it so
        // local submissions work after `/remote off`.
        state.session_taken_over = false;
        state
            .status_panel
            .set_backend(sandbox_display_name(&state.agent_config.operation_backend));
        state.status_panel.set_workspace(&state.workspace);
        Ok(())
    }

    pub async fn remote_status(&self, state: &AppState) -> String {
        let Some(remote) = self.remote.as_ref() else {
            return format!(
                "Backend: {}",
                sandbox_display_name(&state.agent_config.operation_backend)
            );
        };

        let token = match resolve_bearer_token(remote.bearer_token_env.as_deref()) {
            Ok(token) => token,
            Err(error) => return format!("Backend: Remote {}\nHealth: {error}", remote.base_url),
        };
        let client = self.http_client.clone();
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
            "Backend: Remote {}\nSession: {}\nSession open: {}\nHealth: {}",
            remote.base_url, state.session_id, self.remote_session_open, health
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
        command_context: Option<agent_types::chat::CommandContext>,
        chain_depth: usize,
    ) -> Result<(), String> {
        let remote = self
            .remote
            .clone()
            .ok_or_else(|| "remote backend is not configured".to_string())?;
        let token = resolve_bearer_token(remote.bearer_token_env.as_deref())?;
        let client = self.http_client.clone();
        let client_id = self.client_id.clone();

        if !self.remote_session_open {
            let open_request = self.remote_session_open_request(state)?;
            post_json(
                &client,
                &remote,
                token.as_deref(),
                "/api/v1/runtimes/open",
                &open_request,
            )
            .await?;
            self.remote_session_open = true;
        }

        let turn_request =
            self.remote_turn_request(state, prompt.clone(), command_context, chain_depth)?;

        state.chat_state.stick_to_bottom = true;
        self.request_start = Some(std::time::Instant::now());
        self.first_token_latency_recorded = false;
        if append_user_message {
            state.chat_state.messages.push(Message::user(prompt));
            state.chat_state.input.reset();
        }
        let _ = record_remote_session(
            &state.session_id,
            &remote.base_url,
            remote.bearer_token_env.clone(),
            state
                .chat_state
                .messages
                .iter()
                .find(|message| message.role == crate::chat::MessageRole::User)
                .map(|message| message.content.as_str()),
        );
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
                client_id,
                turn_request,
                updates_tx,
                interaction_rx,
            )
            .await;
        });

        Ok(())
    }

    /// `POST /api/v1/runtimes/close` — destroy the session on the daemon.
    /// Returns `Err` on transport / non-2xx so the caller can surface the
    /// failure instead of falsely reporting success. `remote_session_open`
    /// is only cleared on success: on failure the daemon may still hold the
    /// session (and our lease on it), so a subsequent `disconnect_remote`
    /// still needs to detach.
    pub async fn close_remote_session(&mut self, session_id: &str) -> Result<(), String> {
        let Some(remote) = self.remote.clone() else {
            return Ok(());
        };
        let Ok(token) = resolve_bearer_token(remote.bearer_token_env.as_deref()) else {
            return Ok(());
        };
        let client = self.http_client.clone();
        let result = post_json(
            &client,
            &remote,
            token.as_deref(),
            "/api/v1/runtimes/close",
            &RuntimeCloseRequest {
                session_id: session_id.to_string(),
                client_id: Some(self.client_id.clone()),
            },
        )
        .await;
        if result.is_ok() {
            self.remote_session_open = false;
        }
        result
    }

    /// `POST /api/v1/runtimes/detach` — release this TUI's attach lease on
    /// `session_id` without destroying the session or its backend (used by
    /// exit / `/new` / `/remote off`). Awaits the HTTP call so callers'
    /// `tokio::time::timeout` wrappers bound the wait; errors surface as
    /// `String` but never block shutdown (callers log and continue).
    pub async fn detach_remote_session(&self, session_id: &str) -> Result<(), String> {
        let Some(remote) = self.remote.clone() else {
            return Ok(());
        };
        let Ok(token) = resolve_bearer_token(remote.bearer_token_env.as_deref()) else {
            return Ok(());
        };
        let client = self.http_client.clone();
        let session_id = session_id.to_string();
        post_json(
            &client,
            &remote,
            token.as_deref(),
            "/api/v1/runtimes/detach",
            &RuntimeDetachRequest {
                session_id,
                client_id: Some(self.client_id.clone()),
            },
        )
        .await
    }

    /// Detach bounded by [`EXIT_RPC_TIMEOUT`] so an unreachable daemon cannot
    /// freeze the TUI event loop. Errors are logged (with `context` to identify
    /// the caller path) and swallowed — every caller treats detach as
    /// best-effort.
    pub(crate) async fn detach_remote_session_bounded(&self, session_id: &str, context: &str) {
        match tokio::time::timeout(EXIT_RPC_TIMEOUT, self.detach_remote_session(session_id)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, context = %context, "remote session detach failed");
            }
            Err(_) => {
                tracing::warn!(
                    context = %context,
                    timeout = ?EXIT_RPC_TIMEOUT,
                    "remote session detach timed out"
                );
            }
        }
    }

    /// `POST /api/v1/runtimes/heartbeat` — renew this TUI's attach lease,
    /// called every 15 s by the App's event loop. Returns `Ok(())` while the
    /// caller is still the holder, or `Err(TakenOver)` when the daemon reports
    /// another TUI holds the lease (the App then sets
    /// `state.session_taken_over = true` and refuses further submissions).
    pub async fn heartbeat_remote_session(&self, session_id: &str) -> Result<(), HeartbeatError> {
        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };
        let token = resolve_bearer_token(remote.bearer_token_env.as_deref())
            .map_err(|error| HeartbeatError::Network(error))?;
        let client = self.http_client.clone();
        let url = format!("{}/api/v1/runtimes/heartbeat", remote.base_url);
        let mut request = client.post(url).json(&RuntimeHeartbeatRequest {
            session_id: session_id.to_string(),
            client_id: Some(self.client_id.clone()),
            client_pid: Some(std::process::id()),
            client_hostname: self.client_hostname.clone(),
        });
        if let Some(token) = token.as_deref() {
            request = request.bearer_auth(token);
        }
        // Bound send by 3 s so an unreachable daemon cannot freeze the
        // event loop; the 15 s interval leaves a comfortable retry budget.
        let response = match tokio::time::timeout(HEARTBEAT_RPC_TIMEOUT, request.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(e)) => return Err(HeartbeatError::Network(e.to_string())),
            Err(_) => {
                return Err(HeartbeatError::Network(format!(
                    "heartbeat timed out after {HEARTBEAT_RPC_TIMEOUT:?}"
                )));
            }
        };
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(());
        }
        if response.status() == StatusCode::CONFLICT {
            // Daemon says another TUI holds the lease. Parse the same 409
            // body shape as `/runtimes/open` so we can surface `stale`: a
            // stale lease means the recorded holder is presumed dead and the
            // caller may reclaim via `open_remote_session_with_record`.
            let body = read_response_text_bounded(response).await;
            let parsed = parse_session_attached_body(&body);
            let detail = format_remote_attached_error(&parsed);
            tracing::warn!(
                session_id = %session_id,
                status = %StatusCode::CONFLICT,
                stale = ?parsed.stale,
                holder_client_id = ?parsed.holder_client_id,
                "heartbeat rejected: session taken over by another client"
            );
            return Err(HeartbeatError::TakenOver {
                detail,
                stale: parsed.stale,
            });
        }
        let status = response.status();
        let body = read_response_text_bounded(response).await;
        tracing::warn!(
            session_id = %session_id,
            status = %status,
            body = %body,
            "heartbeat failed with non-2xx, non-409 status"
        );
        Err(HeartbeatError::Network(format!("HTTP {status} {body}")))
    }

    /// Calls `/api/v1/runtimes/open` for the current `state.session_id` and
    /// returns the parsed `SessionRecord`. Idempotent on the daemon side:
    /// if the session already exists, the daemon returns it (with messages
    /// intact) so the TUI can restore context after a switch. Sets
    /// `remote_session_open = true` on success.
    ///
    /// On 409 (`SessionAttachedByAnotherClient`) the error string carries a
    /// user-readable hint; the caller decides whether to auto-retry (stale
    /// lease → safe to take over) or surface the error.
    pub async fn open_remote_session_with_record(
        &mut self,
        state: &mut crate::app_state::AppState,
    ) -> Result<crate::gateway::SessionRecord, String> {
        let remote = self
            .remote
            .clone()
            .ok_or_else(|| "remote backend is not configured".to_string())?;
        let token = resolve_bearer_token(remote.bearer_token_env.as_deref())?;
        let client = self.http_client.clone();
        let open_request = self.remote_session_open_request(state)?;
        let url = format!("{}/api/v1/runtimes/open", remote.base_url);
        let mut request = client.post(url).json(&open_request);
        if let Some(token) = token.as_ref() {
            request = request.bearer_auth(token);
        }
        // Bound send by 15 s — `open` may involve backend provisioning (e.g.
        // leasing an e2b sandbox), but must not hang on the OS's 75 s+ TCP
        // timeout when the daemon is unreachable.
        let response = match tokio::time::timeout(OPEN_RPC_TIMEOUT, request.send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => return Err(format!("remote open failed: {error}")),
            Err(_) => return Err(format!("remote open timed out after {OPEN_RPC_TIMEOUT:?}")),
        };
        let status = response.status();
        if status == StatusCode::CONFLICT {
            // Daemon says the session is held by another client. Body shape
            // matches `map_session_error`'s JSON (`{"error": "<Display>"}`),
            // which already includes holder/stale info; relay it verbatim.
            let body = read_response_text_bounded(response).await;
            let parsed = parse_session_attached_body(&body);
            return Err(format_remote_attached_error(&parsed));
        }
        if !status.is_success() {
            let body = read_response_text_bounded(response).await;
            return Err(format!("remote open failed: HTTP {status} {body}"));
        }
        let record: crate::gateway::SessionRecord = response
            .json()
            .await
            .map_err(|error| format!("failed to parse session record: {error}"))?;
        self.remote_session_open = true;
        Ok(record)
    }

    pub fn cancel_remote_turn(&self, session_id: String) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let Ok(token) = resolve_bearer_token(remote.bearer_token_env.as_deref()) else {
            return;
        };
        let client = self.http_client.clone();
        let client_id = self.client_id.clone();
        tokio::spawn(async move {
            let _ = post_json(
                &client,
                &remote,
                token.as_deref(),
                "/api/v1/runtimes/cancel",
                &RuntimeCancelRequest {
                    session_id,
                    client_id: Some(client_id),
                },
            )
            .await;
        });
    }

    fn remote_session_open_request(&self, state: &AppState) -> Result<RuntimeOpenRequest, String> {
        Self::remote_session_open_request_for(
            state,
            self.remote.as_ref().map(|remote| remote.base_url.clone()),
            self.client_hostname.clone(),
        )
    }

    fn remote_session_open_request_for(
        state: &AppState,
        base_url: Option<String>,
        client_hostname: Option<String>,
    ) -> Result<RuntimeOpenRequest, String> {
        let sender_id = super::runtime_request::resolve_agent_id(None, None, &state.agent_config)?;
        Ok(RuntimeOpenRequest {
            session_id: state.session_id.clone(),
            conversation_id: state.session_id.clone(),
            sender_id,
            entry: super::runtime_request::tui_entry_context(state, base_url),
            channel: None,
            channel_instance_id: None,
            llm: Some(super::runtime_request::llm_runtime_config_from_state(state)),
            workspace: None,
            skills: None,
            client_id: Some(state.client_id.clone()),
            client_pid: Some(std::process::id()),
            client_hostname,
        })
    }

    fn remote_turn_request(
        &self,
        state: &AppState,
        text: String,
        command_context: Option<agent_types::chat::CommandContext>,
        chain_depth: usize,
    ) -> Result<RuntimeTurnRequest, String> {
        let sender_id = super::runtime_request::resolve_agent_id(None, None, &state.agent_config)?;
        Ok(RuntimeTurnRequest {
            session_id: state.session_id.clone(),
            entry: super::runtime_request::tui_entry_context(
                state,
                self.remote.as_ref().map(|r| r.base_url.clone()),
            ),
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
            llm: Some(super::runtime_request::llm_runtime_config_from_state(state)),
            workspace: None,
            skills: None,
            command_context,
            chain_depth,
            client_id: Some(state.client_id.clone()),
        })
    }
}

async fn run_remote_stream(
    client: reqwest::Client,
    remote: RemoteRuntimeConfig,
    token: Option<String>,
    client_id: String,
    turn_request: RuntimeTurnRequest,
    updates_tx: UnboundedSender<SessionTurnUpdate>,
    mut interaction_rx: UnboundedReceiver<UserPromptResult>,
) {
    let url = format!("{}/api/v1/runtimes/input", remote.base_url);
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
        let body = read_response_text_bounded(response).await;
        let _ = updates_tx.send(SessionTurnUpdate::Err(format!(
            "remote input failed: HTTP {status} {body}"
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
                        &client_id,
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
    client_id: &str,
    session_id: &str,
    updates_tx: &UnboundedSender<SessionTurnUpdate>,
    interaction_rx: &mut UnboundedReceiver<UserPromptResult>,
) {
    match event {
        RemoteSseEvent::TurnStart { agent_id, turn } => {
            let _ = updates_tx.send(SessionTurnUpdate::TurnStart {
                agent_id: AgentId(agent_id),
                turn,
            });
        }
        RemoteSseEvent::TextDelta {
            agent_id, snapshot, ..
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::SetAssistantContent {
                agent_id: AgentId(agent_id.unwrap_or_else(|| "cli-agent".to_string())),
                text: snapshot,
            });
        }
        RemoteSseEvent::ThinkingDelta {
            agent_id, snapshot, ..
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::SetAssistantThinking {
                agent_id: AgentId(agent_id.unwrap_or_else(|| "cli-agent".to_string())),
                text: snapshot,
            });
        }
        RemoteSseEvent::ToolResult {
            agent_id,
            call_id,
            tool_name,
            output_preview,
            is_error,
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::Tool {
                agent_id: AgentId(agent_id.unwrap_or_else(|| "cli-agent".to_string())),
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
        RemoteSseEvent::ToolFileChange {
            call_id,
            file_path,
            additions,
            deletions,
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::ToolFileChange {
                call_id,
                delta: crate::chat::FileChangeDelta {
                    file_path,
                    additions,
                    deletions,
                },
            });
        }
        RemoteSseEvent::PlanUpdate { title, items } => {
            let _ = updates_tx.send(SessionTurnUpdate::PlanUpdate {
                snapshot: TodoSnapshotUpdate { title, items },
            });
        }
        RemoteSseEvent::SubagentSpawn {
            agent_id,
            parent_agent_id,
            title,
            description,
            task_goal,
        } => {
            let _ = updates_tx.send(SessionTurnUpdate::SubagentSpawn {
                metadata: SpawnSubagentMetadata {
                    agent_id,
                    parent_agent_id,
                    title,
                    description,
                    task_goal,
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
                let _ = post_json(
                    client,
                    remote,
                    token,
                    "/api/v1/runtimes/interaction",
                    &RuntimeInteractionRequest {
                        session_id: session_id.to_string(),
                        response,
                        // Pass through `client_id` so the daemon's lease
                        // guard on /interaction can attribute the response
                        // to this TUI's lease.
                        client_id: Some(client_id.to_string()),
                    },
                )
                .await;
                break;
            }
        }
        RemoteSseEvent::Done {
            total_tokens,
            prompt_tokens,
            completion_tokens,
            estimated_input_tokens,
            messages,
            actions,
            ..
        } => {
            // Send HookActions BEFORE Done: `poll_stream_updates` clears
            // `stream_rx` on `Done` and exits the receive loop, so any update
            // sent after `Done` would sit undrained in the channel buffer
            // (actions would be silently dropped).
            if !actions.is_empty() {
                let _ = updates_tx.send(SessionTurnUpdate::HookActions(actions));
            }
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
    // Safety-net bound so no `post_json` caller can hang on an unreachable
    // daemon. Tighter caller-side timeouts (e.g. `close_sessions`'s 5 s) fire
    // first; this is the last line of defense.
    let response = match tokio::time::timeout(POST_JSON_SAFETY_TIMEOUT, request.send()).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => return Err(error.to_string()),
        Err(_) => {
            return Err(format!(
                "HTTP POST {path} timed out after {POST_JSON_SAFETY_TIMEOUT:?}"
            ))
        }
    };
    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let body = read_response_text_bounded(response).await;
        Err(format!("HTTP {status} {body}"))
    }
}

/// Parse the 409 body emitted by `map_session_error` for a
/// `SessionAttachedByAnotherClient` response, extracting the structured
/// fields (`holder_client_id`, `holder_hostname`, `holder_pid`, `stale`).
/// If the daemon later evolves the shape, missing fields are reported as
/// `None` and `format_remote_attached_error` falls back to the raw body —
/// callers are never blocked by a parse regression.
struct SessionAttachedInfo {
    raw: String,
    holder_client_id: Option<String>,
    holder_hostname: Option<String>,
    holder_pid: Option<u32>,
    stale: Option<bool>,
}

fn parse_session_attached_body(body: &str) -> SessionAttachedInfo {
    let raw = body.to_string();
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(body) else {
        return SessionAttachedInfo {
            raw,
            holder_client_id: None,
            holder_hostname: None,
            holder_pid: None,
            stale: None,
        };
    };
    // `kind` is the contract marker; if absent or mismatched, surface the
    // raw body rather than guessing.
    let kind = parsed.get("kind").and_then(|v| v.as_str());
    if kind != Some("session_attached_by_another_client") {
        return SessionAttachedInfo {
            raw,
            holder_client_id: None,
            holder_hostname: None,
            holder_pid: None,
            stale: None,
        };
    }
    SessionAttachedInfo {
        raw,
        holder_client_id: parsed
            .get("holder_client_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        holder_hostname: parsed
            .get("holder_hostname")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        holder_pid: parsed
            .get("holder_pid")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok()),
        stale: parsed.get("stale").and_then(|v| v.as_bool()),
    }
}

fn format_remote_attached_error(info: &SessionAttachedInfo) -> String {
    // Prefer the full `cid@host (pid=…)` triple, fall back to just `cid`,
    // fall back to nothing.
    let holder_line = match (
        &info.holder_client_id,
        &info.holder_hostname,
        info.holder_pid,
    ) {
        (Some(cid), Some(host), Some(pid)) => {
            format!("\n  Held by: {cid}@{host} (pid={pid})")
        }
        (Some(cid), _, _) => format!("\n  Held by: {cid}"),
        _ => String::new(),
    };
    // `None` surfaces the raw body so the user sees what the daemon returned
    // instead of a guessed hint.
    let stale_hint = match info.stale {
        Some(true) => "\nThe lease is stale (the holder appears to have crashed). \
             Re-running /sessions or /remote should succeed automatically."
            .to_string(),
        Some(false) => "\nThe lease is still live. Stop the other xiaoo process, \
             then re-run `/remote <url>` — or switch to a different session \
             via /sessions."
            .to_string(),
        None => format!("\nRaw error: {}", info.raw),
    };
    format!(
        "Remote session is currently attached to another xiaoo process.\
         {holder_line}{stale_hint}"
    )
}

/// Per-call timeout for reading an HTTP response *body* (error / status
/// line text). `Response::text()` reads the body stream without an upper
/// bound, so a wedged / malicious daemon could stream bytes forever and
/// freeze the event loop. Error bodies are small (<1 KiB typically), so 2 s
/// is a generous upper bound.
const RESPONSE_BODY_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Read a response body as text, bounded by [`RESPONSE_BODY_READ_TIMEOUT`].
/// Returns the empty string on timeout / read error so callers'
/// `format!("HTTP {status} {body}")` paths keep working without per-call-site
/// error plumbing (the `send()` timeout already surfaces the "daemon
/// unreachable" case; a body-read timeout is best-effort).
async fn read_response_text_bounded(response: reqwest::Response) -> String {
    match tokio::time::timeout(RESPONSE_BODY_READ_TIMEOUT, response.text()).await {
        Ok(Ok(text)) => text,
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "response body read failed; using empty body");
            String::new()
        }
        Err(_) => {
            tracing::warn!(
                timeout = ?RESPONSE_BODY_READ_TIMEOUT,
                "response body read timed out; using empty body (daemon may be streaming an unbounded body)"
            );
            String::new()
        }
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
                    id: "yes".to_string(),
                    label: "Yes".to_string(),
                    description: None,
                },
                PromptChoice {
                    id: "no".to_string(),
                    label: "No".to_string(),
                    description: None,
                },
            ],
            allow_custom_input: false,
            multi_select: false,
            default_index: Some(0),
            is_secret: false,
        },
        InteractionRequest::TextInput {
            prompt, is_secret, ..
        } => PromptRequest {
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
            is_secret: *is_secret,
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
            is_secret: false, // Choice type does not need password hiding
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
                allowed: choice_id == "yes",
            })
        }
        (
            InteractionRequest::TextInput { is_secret, .. },
            PromptResolution::Single { supplement, .. },
        ) => {
            // For secret inputs, use display_value to hide the password in messages
            let display_value = if *is_secret {
                Some("<SECRET>".to_string())
            } else {
                None
            };
            Some(InteractionResponse::Text {
                value: supplement,
                display_value,
            })
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
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: None,
            display_value: None,
        },
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
            RemoteSseEvent::TextDelta {
                agent_id,
                delta,
                snapshot,
            } => {
                assert!(agent_id.is_none());
                assert_eq!(delta, "he");
                assert_eq!(snapshot, "he");
            }
            other => panic!("unexpected event: {other:?}"),
        }
        assert_eq!(buffer, "rest");
    }

    #[test]
    fn parses_sse_frame_with_agent_id() {
        let parsed = parse_sse_frame(
            "event: text_delta\ndata: {\"type\":\"text_delta\",\"agent_id\":\"child\",\"delta\":\"he\",\"snapshot\":\"he\"}",
        )
        .expect("parse")
        .expect("event");

        match parsed {
            RemoteSseEvent::TextDelta { agent_id, .. } => {
                assert_eq!(agent_id.as_deref(), Some("child"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn ignores_keepalive_frame() {
        let parsed = parse_sse_frame(": keepalive").expect("parse");
        assert!(parsed.is_none());
    }
}
