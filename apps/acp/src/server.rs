use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as SyncMutex, OnceLock,
    },
    time::Duration,
};

use agent_client_protocol::{
    schema::{v1 as acp, ProtocolVersion},
    Agent, ByteStreams, Client, ConnectionTo, Error, Responder,
};
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, Mutex, Notify};
use tokio_util::{
    compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt},
    sync::CancellationToken,
};
use xiaoo_api::{
    chat::AgentId,
    events::{LoopEndSummary, LoopEventSink, ToolResultEvent},
};
use xiaoo_shared::gateway::{controlled_session::ControlledTextSession, TurnOutcome};

use crate::{Config, COMMIT, VERSION};

const PROFILE_META: &str = "org.openeuler.xiaoo/acp";
const BUILD_META: &str = "org.openeuler.xiaoo/build";
const USAGE_META: &str = "org.openeuler.xiaoo/usage";
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const UPDATE_BUFFER: usize = 256;

struct Server {
    config: Config,
    client_capabilities: OnceLock<acp::ClientCapabilities>,
    sessions: Mutex<HashMap<acp::SessionId, Arc<Session>>>,
    disconnect: CancellationToken,
}

struct Session {
    runtime: ControlledTextSession,
    state: Mutex<SessionState>,
    idle: Notify,
}

struct SessionState {
    profile: String,
    active: Option<CancellationToken>,
    closing: bool,
}

fn invalid(message: &str) -> Error {
    Error::new(-32602, message)
}

impl Server {
    fn new(config: Config) -> Self {
        Self {
            config,
            client_capabilities: OnceLock::new(),
            sessions: Mutex::new(HashMap::new()),
            disconnect: CancellationToken::new(),
        }
    }

    fn initialize(
        &self,
        request: acp::InitializeRequest,
    ) -> Result<acp::InitializeResponse, Error> {
        if self
            .client_capabilities
            .set(request.client_capabilities)
            .is_err()
        {
            return Err(Error::new(-32600, "connection is already initialized"));
        }

        let capabilities = acp::AgentCapabilities::new()
            .session_capabilities(
                acp::SessionCapabilities::new().close(acp::SessionCloseCapabilities::new()),
            )
            .meta(meta(
                PROFILE_META,
                json!({"mode": "controlled-text", "version": 1}),
            ));
        Ok(acp::InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(capabilities)
            .agent_info(acp::Implementation::new("xiaoo-acp", VERSION))
            .meta(meta(BUILD_META, json!({"commit": COMMIT}))))
    }

    fn require_initialize(&self) -> Result<(), Error> {
        if self.client_capabilities.get().is_some() {
            Ok(())
        } else {
            Err(Error::new(-32600, "initialize the connection first"))
        }
    }

    async fn session(&self, id: &acp::SessionId) -> Result<Arc<Session>, Error> {
        self.require_initialize()?;
        self.sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| invalid("unknown session"))
    }

    fn options(&self, profile: &str) -> Vec<acp::SessionConfigOption> {
        let values = self
            .config
            .models
            .keys()
            .map(|id| acp::SessionConfigSelectOption::new(id.clone(), id.clone()))
            .collect::<Vec<_>>();
        vec![
            acp::SessionConfigOption::select("model", "Model", profile.to_string(), values)
                .category(acp::SessionConfigOptionCategory::Model),
        ]
    }

    async fn new_session(
        &self,
        request: acp::NewSessionRequest,
    ) -> Result<acp::NewSessionResponse, Error> {
        self.require_initialize()?;
        if !request.mcp_servers.is_empty() {
            return Err(invalid("the controlled profile requires empty mcpServers"));
        }
        if !request.additional_directories.is_empty() {
            return Err(invalid("additional roots are unsupported"));
        }
        if !request.cwd.is_absolute() {
            return Err(invalid("cwd must be absolute"));
        }

        let selected = request
            .meta
            .as_ref()
            .and_then(|meta| meta.get(PROFILE_META));
        let profile = match selected {
            Some(Value::Object(options)) => match options.get("modelProfile") {
                Some(Value::String(profile)) => profile,
                None => &self.config.default_model,
                _ => return Err(invalid("modelProfile must be a profile identifier")),
            },
            None => &self.config.default_model,
            _ => return Err(invalid("invalid xiaoO session options")),
        };
        let configured = self
            .config
            .models
            .get(profile)
            .ok_or_else(|| invalid("unknown model profile"))?;
        let id = uuid::Uuid::new_v4().to_string();
        let runtime = ControlledTextSession::open(id.clone(), request.cwd, configured)
            .await
            .map_err(|_| Error::new(-32000, "session model or runtime could not be initialized"))?;
        let id = acp::SessionId::new(id);
        self.sessions.lock().await.insert(
            id.clone(),
            Arc::new(Session {
                runtime,
                state: Mutex::new(SessionState {
                    profile: profile.clone(),
                    active: None,
                    closing: false,
                }),
                idle: Notify::new(),
            }),
        );
        Ok(acp::NewSessionResponse::new(id).config_options(self.options(profile)))
    }

    async fn select_model(
        &self,
        request: acp::SetSessionConfigOptionRequest,
    ) -> Result<acp::SetSessionConfigOptionResponse, Error> {
        let session = self.session(&request.session_id).await?;
        let mut state = session.state.lock().await;
        require_idle(&state)?;
        let value = request
            .value
            .as_value_id()
            .ok_or_else(|| invalid("model must be a profile identifier"))?;
        if request.config_id.to_string() != "model" {
            return Err(invalid("unknown session option"));
        }
        let profile = value.to_string();
        let configured = self
            .config
            .models
            .get(&profile)
            .ok_or_else(|| invalid("unknown model profile"))?;
        session
            .runtime
            .select_model(configured)
            .await
            .map_err(|_| Error::new(-32000, "session model could not be initialized"))?;
        state.profile = profile;
        Ok(acp::SetSessionConfigOptionResponse::new(
            self.options(&state.profile),
        ))
    }

    async fn prompt(
        &self,
        request: acp::PromptRequest,
        responder: Responder<acp::PromptResponse>,
        connection: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        let session = match self.session(&request.session_id).await {
            Ok(session) => session,
            Err(error) => return responder.respond_with_error(error),
        };
        let text = match prompt_text(request.prompt) {
            Ok(text) => text,
            Err(error) => return responder.respond_with_error(error),
        };
        let cancel = self.disconnect.child_token();
        {
            let mut state = session.state.lock().await;
            if let Err(error) = require_idle(&state) {
                return responder.respond_with_error(error);
            }
            state.active = Some(cancel.clone());
        }
        tokio::spawn(async move {
            let result = run_prompt(&session, text, cancel, &request.session_id, &connection).await;
            let mut state = session.state.lock().await;
            // Serialize terminal response and idle admission for this session.
            let _ = match result {
                Ok(response) => responder.respond(response),
                Err(error) => responder.respond_with_error(error),
            };
            state.active = None;
            session.idle.notify_waiters();
        });
        Ok(())
    }

    async fn cancel(&self, id: &acp::SessionId) -> Result<(), Error> {
        let session = self.session(id).await?;
        if let Some(active) = &session.state.lock().await.active {
            active.cancel();
        }
        Ok(())
    }

    async fn close(&self, id: &acp::SessionId) -> Result<acp::CloseSessionResponse, Error> {
        let session = self.session(id).await?;
        {
            let mut state = session.state.lock().await;
            if state.closing {
                return Err(Error::new(-32000, "session is closing"));
            }
            state.closing = true;
            if let Some(active) = &state.active {
                active.cancel();
            }
        }
        let closed = tokio::time::timeout(CLOSE_TIMEOUT, async {
            loop {
                let idle = session.idle.notified();
                if session.state.lock().await.active.is_none() {
                    break;
                }
                idle.await;
            }
            session.runtime.close().await
        })
        .await;
        self.sessions.lock().await.remove(id);
        match closed {
            Ok(Ok(())) => Ok(acp::CloseSessionResponse::new()),
            Ok(Err(_)) => Err(Error::new(-32000, "session close failed")),
            Err(_) => Err(Error::new(-32000, "session close timed out")),
        }
    }
}

fn require_idle(state: &SessionState) -> Result<(), Error> {
    if state.closing || state.active.is_some() {
        Err(Error::new(-32000, "session is busy"))
    } else {
        Ok(())
    }
}

fn meta(key: &str, value: Value) -> acp::Meta {
    Map::from_iter([(key.to_string(), value)])
}

fn prompt_text(blocks: Vec<acp::ContentBlock>) -> Result<String, Error> {
    let text = blocks
        .into_iter()
        .map(|block| match block {
            acp::ContentBlock::Text(block) => Ok(block.text),
            acp::ContentBlock::ResourceLink(link) => Ok(format!("{} ({})", link.name, link.uri)),
            _ => Err(invalid("the text profile supports text and resource links")),
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n\n");
    if text.trim().is_empty() {
        return Err(invalid("prompt must contain content"));
    }
    Ok(text)
}

struct TurnOutput {
    updates: mpsc::Sender<acp::SessionUpdate>,
    cancel: CancellationToken,
    delivery_failed: Arc<AtomicBool>,
    text: SyncMutex<String>,
    reasoning: SyncMutex<String>,
}

impl TurnOutput {
    fn emit(&self, update: acp::SessionUpdate) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.updates.try_send(update) {
            self.delivery_failed.store(true, Ordering::Release);
            self.cancel.cancel();
        }
    }
}

impl TurnOutput {
    fn emit_snapshot(
        &self,
        previous: &SyncMutex<String>,
        current: &str,
        update: fn(acp::ContentChunk) -> acp::SessionUpdate,
    ) {
        let mut previous = previous.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(delta) = current.strip_prefix(previous.as_str()) {
            if !delta.is_empty() {
                self.emit(update(acp::ContentChunk::new(delta.into())));
                previous.push_str(delta);
            }
        } else if !current.is_empty() && !previous.starts_with(current) {
            // ACP updates append text. A provider retry cannot replace what was
            // already sent to the client.
            self.delivery_failed.store(true, Ordering::Release);
            self.cancel.cancel();
        }
    }
}

impl LoopEventSink for TurnOutput {
    fn on_turn_start(&self, _: &AgentId, _: u32) {
        for buffer in [&self.text, &self.reasoning] {
            buffer
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clear();
        }
    }

    fn on_assistant_message(&self, _: &AgentId, text: &str) {
        self.emit_snapshot(&self.text, text, acp::SessionUpdate::AgentMessageChunk);
    }

    fn on_assistant_reasoning(&self, _: &AgentId, reasoning: &str) {
        self.emit_snapshot(
            &self.reasoning,
            reasoning,
            acp::SessionUpdate::AgentThoughtChunk,
        );
    }

    fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
    fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
}

async fn run_prompt(
    session: &Session,
    text: String,
    cancel: CancellationToken,
    id: &acp::SessionId,
    connection: &ConnectionTo<Client>,
) -> Result<acp::PromptResponse, Error> {
    let (sender, mut receiver) = mpsc::channel(UPDATE_BUFFER);
    let delivery_failed = Arc::new(AtomicBool::new(false));
    let output = Arc::new(TurnOutput {
        updates: sender,
        cancel: cancel.clone(),
        delivery_failed: delivery_failed.clone(),
        text: SyncMutex::default(),
        reasoning: SyncMutex::default(),
    });
    let turn = session.runtime.prompt(text, output, cancel.clone());
    tokio::pin!(turn);

    let emit = |update| {
        if connection
            .send_notification(acp::SessionNotification::new(id.clone(), update))
            .is_err()
        {
            cancel.cancel();
        }
    };
    let (result, usage) = loop {
        tokio::select! {
            result = &mut turn => break result,
            Some(update) = receiver.recv() => emit(update),
        }
    };
    while let Ok(update) = receiver.try_recv() {
        emit(update);
    }
    let usage = serde_json::to_value(usage).map_err(|_| Error::internal_error())?;
    if delivery_failed.load(Ordering::Acquire) {
        return Err(
            Error::new(-32000, "prompt output could not be delivered in order")
                .data(json!({"usage": usage})),
        );
    }
    let reason = match result {
        Ok(TurnOutcome::Complete) => acp::StopReason::EndTurn,
        Ok(TurnOutcome::MaxTurnsReached) => acp::StopReason::MaxTurnRequests,
        Ok(TurnOutcome::BudgetExhausted) => acp::StopReason::MaxTokens,
        Ok(TurnOutcome::Cancelled) => acp::StopReason::Cancelled,
        Err(_) => {
            return Err(Error::new(-32000, "prompt failed").data(json!({"usage": usage})));
        }
    };
    Ok(acp::PromptResponse::new(reason).meta(meta(USAGE_META, usage)))
}

pub async fn serve(config: Config) -> anyhow::Result<()> {
    let server = Arc::new(Server::new(config));
    let init = Arc::clone(&server);
    let new = Arc::clone(&server);
    let select = Arc::clone(&server);
    let prompt = Arc::clone(&server);
    let cancel = Arc::clone(&server);
    let close = Arc::clone(&server);
    let agent = Agent
        .builder()
        .name("xiaoo-acp")
        .on_receive_request(
            async move |request: acp::InitializeRequest,
                        responder: Responder<acp::InitializeResponse>,
                        _| { responder.respond_with_result(init.initialize(request)) },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::NewSessionRequest,
                        responder: Responder<acp::NewSessionResponse>,
                        _| {
                responder.respond_with_result(new.new_session(request).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::SetSessionConfigOptionRequest,
                        responder: Responder<acp::SetSessionConfigOptionResponse>,
                        _| {
                responder.respond_with_result(select.select_model(request).await)
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: acp::PromptRequest,
                        responder: Responder<acp::PromptResponse>,
                        connection| {
                prompt.prompt(request, responder, connection).await
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |request: acp::CancelNotification, _| {
                cancel.cancel(&request.session_id).await
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: acp::CloseSessionRequest,
                        responder: Responder<acp::CloseSessionResponse>,
                        _| {
                responder.respond_with_result(close.close(&request.session_id).await)
            },
            agent_client_protocol::on_receive_request!(),
        );
    let streams = ByteStreams::new(
        tokio::io::stdout().compat_write(),
        tokio::io::stdin().compat(),
    );
    let result = agent.connect_to(streams).await;
    server.disconnect.cancel();
    let sessions = server
        .sessions
        .lock()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut closing = tokio::task::JoinSet::new();
    for id in sessions {
        let server = server.clone();
        closing.spawn(async move { server.close(&id).await });
    }
    while closing.join_next().await.is_some() {}
    result.map_err(|_| anyhow::anyhow!("ACP transport failed"))
}
