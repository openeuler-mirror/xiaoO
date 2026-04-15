use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionControlPlane,
    SessionLifecycleStatus, SessionOpenRequest, SessionRecord, SessionRuntimeBuildInput,
    SessionRuntimeResolveError, SessionRuntimeResolver, SessionService, SessionServiceError,
    SessionStore, SessionStoreError,
};
use agent_contracts::LoopEventSink;
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use subagent::{
    JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControl, SubagentControlError,
};
use tokio::sync::Mutex;

use super::session_supervisor::SessionSupervisor;

pub struct CoreBackedSessionService {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    supervisors: Mutex<HashMap<String, Arc<SessionSupervisor>>>,
}

impl CoreBackedSessionService {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            supervisors: Mutex::new(HashMap::new()),
        }
    }

    async fn get_or_create_supervisor(&self, session: SessionRecord) -> Arc<SessionSupervisor> {
        let mut supervisors = self.supervisors.lock().await;
        if let Some(existing) = supervisors.get(&session.session_id) {
            return existing.clone();
        }

        let supervisor = Arc::new(SessionSupervisor::new(
            self.session_store.clone(),
            self.runtime_resolver.clone(),
            session.clone(),
        ));
        supervisors.insert(session.session_id.clone(), supervisor.clone());
        supervisor
    }

    async fn supervisor_for_session(&self, session_id: &str) -> Option<Arc<SessionSupervisor>> {
        if let Some(existing) = self.supervisors.lock().await.get(session_id).cloned() {
            return Some(existing);
        }

        let session = self.session_store.load(session_id).await?;
        Some(self.get_or_create_supervisor(session).await)
    }

    fn build_session_for_turn(
        request: &AppTurnRequest,
        resolved: &ResolvedSessionRuntime,
    ) -> SessionRecord {
        let now_ms = current_time_ms();
        SessionRecord {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            entry: request.entry.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: resolved.descriptor.agent_id.clone(),
                model: resolved.descriptor.model.clone(),
                system_prompt: resolved.descriptor.system_prompt.clone(),
                feature_flags: resolved.descriptor.feature_flags.clone(),
                token_budget: resolved.descriptor.token_budget.clone(),
                workspace_root: resolved.descriptor.workspace_root.clone(),
                max_turns: resolved.descriptor.max_turns,
            },
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    fn build_session_for_open(
        request: &SessionOpenRequest,
        resolved: &ResolvedSessionRuntime,
    ) -> SessionRecord {
        let now_ms = current_time_ms();
        SessionRecord {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            entry: request.entry.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: resolved.descriptor.agent_id.clone(),
                model: resolved.descriptor.model.clone(),
                system_prompt: resolved.descriptor.system_prompt.clone(),
                feature_flags: resolved.descriptor.feature_flags.clone(),
                token_budget: resolved.descriptor.token_budget.clone(),
                workspace_root: resolved.descriptor.workspace_root.clone(),
                max_turns: resolved.descriptor.max_turns,
            },
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    async fn run_turn_inner(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let existing = self.session_store.load(&request.session_id).await;
        let runtime_input = SessionRuntimeBuildInput::from_turn_request(&request);
        let resolved = self
            .runtime_resolver
            .resolve(&runtime_input, existing.as_ref())
            .await?;

        let seed_session =
            existing.unwrap_or_else(|| Self::build_session_for_turn(&request, &resolved));
        let supervisor = self.get_or_create_supervisor(seed_session).await;
        supervisor.prepare_root_turn(&request, &resolved).await;
        supervisor.run_root_turn(request, event_sink).await
    }
}

#[async_trait]
impl SessionService for CoreBackedSessionService {
    async fn run_turn(
        &self,
        request: AppTurnRequest,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, None).await
    }

    async fn run_turn_with_events(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, event_sink).await
    }
}

#[async_trait]
impl SessionControlPlane for CoreBackedSessionService {
    async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionRecord, SessionServiceError> {
        if let Some(supervisor) = self.supervisor_for_session(&request.session_id).await {
            return Ok(supervisor.snapshot().await);
        }

        let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
        let resolved = self.runtime_resolver.resolve(&runtime_input, None).await?;
        let session = Self::build_session_for_open(&request, &resolved);
        self.session_store.save(session.clone()).await;
        Ok(self
            .get_or_create_supervisor(session)
            .await
            .snapshot()
            .await)
    }

    async fn resume_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        match self.supervisor_for_session(session_id).await {
            Some(supervisor) => Ok(Some(supervisor.snapshot().await)),
            None => Ok(None),
        }
    }

    async fn force_close_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        if let Some(supervisor) = self.supervisor_for_session(session_id).await {
            return Ok(Some(supervisor.force_close().await));
        }

        let Some(mut existing) = self.session_store.load(session_id).await else {
            return Ok(None);
        };
        existing.status = SessionLifecycleStatus::Closed;
        existing.updated_at_ms = current_time_ms();
        self.session_store.save(existing.clone()).await;
        Ok(Some(existing))
    }
}

#[async_trait]
impl SubagentControl for CoreBackedSessionService {
    async fn spawn(
        &self,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError> {
        let Some(supervisor) = self.supervisor_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        supervisor.spawn_subagent(request).await
    }

    async fn join(
        &self,
        request: JoinSubagentRequest,
    ) -> Result<JoinSubagentResult, SubagentControlError> {
        let Some(supervisor) = self.supervisor_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        supervisor.join_subagent(request).await
    }
}

impl From<SessionRuntimeResolveError> for SessionServiceError {
    fn from(value: SessionRuntimeResolveError) -> Self {
        Self::RuntimeResolve {
            message: value.to_string(),
        }
    }
}

impl From<SessionStoreError> for SessionServiceError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore {
            message: value.to_string(),
        }
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
