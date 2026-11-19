use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_contracts::backend::capability::{
    exec::{ExecRequest, ExecResult, OperationExec},
    export::{ExportFileRequest, OperationExport},
    filesystem::{
        OperationFileSystem, ReadBytesRequest, TempPathRequest, WriteBytesOutcome,
        WriteBytesRequest,
    },
    path::{OperationPathResolver, ResolvePathRequest},
    search::{GlobRequest, GrepRequest, GrepResult, OperationSearch},
};
use agent_contracts::backend::{
    BackendId, BackendInstance, BackendInstanceId, BackendLifecycleState, BackendPath,
    BackendProviderKind, OperationBackend, OperationBackendCapabilities, OperationError, PathStat,
    SharedExportedFileHandle,
};
use agent_contracts::{LlmProvider, LoopEventSink, ProviderCapabilities};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};
use agent_types::{LlmError, LlmRequest, LlmResponse, StreamChunk};
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tool::EmptyToolRegistry;
use xiaoo_api::chat::AgentId;

use crate::backend::BackendManager;

use super::llm_assembly::{build_llm_provider, LlmAssemblyInput};
use super::{
    AppBootstrap, GatewayEntryContext, InMemorySessionStore, ResolvedSessionRuntime,
    SessionControlPlane, SessionOpenRequest, SessionRecord, SessionRuntimeBindings,
    SessionRuntimeBuildInput, SessionRuntimeDescriptor, SessionRuntimeResolveError,
    SessionRuntimeResolver, SessionService, TurnOutcome,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlledModelProfile {
    pub provider: String,
    pub model: String,
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key_file: Option<PathBuf>,
    pub context_window: Option<u32>,
}

impl ControlledModelProfile {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.provider.trim().is_empty() && !self.model.trim().is_empty(),
            "profile provider and model are required"
        );
        match (&self.api_key_env, &self.api_key_file) {
            (Some(name), None) if !name.is_empty() && !name.contains(['=', '\0']) => (),
            (None, Some(path)) if path.is_absolute() => (),
            _ => anyhow::bail!(
                "profile requires one credential environment variable or absolute file reference"
            ),
        }
        anyhow::ensure!(
            self.context_window.unwrap_or(32_768) >= 4096,
            "profile context window must be at least 4096"
        );
        Ok(())
    }

    async fn resolve(&self, usage: Arc<Mutex<TurnUsage>>) -> anyhow::Result<ResolvedModel> {
        self.validate()?;
        let key = match (&self.api_key_env, &self.api_key_file) {
            (Some(name), None) => std::env::var(name).ok(),
            (None, Some(path)) => tokio::fs::read_to_string(path).await.ok(),
            _ => None,
        }
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("model credential unavailable"))?;
        let (provider, context_window) = build_llm_provider(LlmAssemblyInput {
            provider: self.provider.clone(),
            model: self.model.clone(),
            api_base: self.api_base.clone(),
            api_key: Some(key.trim().to_string()),
            api_key_env: None,
            request_headers: Default::default(),
            context_window_override: Some(self.context_window.unwrap_or(32_768)),
            agent_id: None,
        })
        .await
        .map_err(|_| anyhow::anyhow!("model configuration could not be resolved"))?;
        let metered = MeteredProvider {
            inner: provider.inner(),
            usage,
        };
        Ok(ResolvedModel {
            name: self.model.clone(),
            context_window: context_window as usize,
            provider: Arc::new(LlmProviderWrapper::new(Arc::new(metered), None, None)),
        })
    }
}

struct ResolvedModel {
    name: String,
    context_window: usize,
    provider: Arc<LlmProviderWrapper>,
}

#[derive(Default)]
struct TurnUsage {
    input: usize,
    output: usize,
    reported: bool,
    incomplete: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlledTurnUsage {
    version: u8,
    input_tokens: Option<usize>,
    output_tokens: Option<usize>,
    status: &'static str,
}

struct MeteredProvider {
    inner: Arc<dyn LlmProvider>,
    usage: Arc<Mutex<TurnUsage>>,
}

impl MeteredProvider {
    fn observe(&self, result: &Result<LlmResponse, LlmError>) {
        let mut meter = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        if let Ok(response) = result {
            let usage = &response.message.usage;
            // Providers without usage normalize it to an all-zero Usage.
            if usage.total_tokens != 0 || usage.prompt_tokens != 0 || usage.completion_tokens != 0 {
                meter.reported = true;
                meter.input = meter.input.saturating_add(usage.prompt_tokens);
                meter.output = meter.output.saturating_add(usage.completion_tokens);
                return;
            }
        }
        meter.incomplete = true;
    }
}

#[async_trait]
impl LlmProvider for MeteredProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let result = self.inner.complete(request).await;
        self.observe(&result);
        result
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let result = self.inner.complete_stream(request, on_chunk).await;
        self.observe(&result);
        result
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        self.inner.capabilities()
    }
}

struct ControlledResolver {
    model: RwLock<ResolvedModel>,
    cwd: PathBuf,
}

fn no_hooks() -> HookerRegistryConfig {
    HookerRegistryConfig {
        default: HookerDefaultMode::None,
        ..Default::default()
    }
}

#[async_trait]
impl SessionRuntimeResolver for ControlledResolver {
    async fn resolve(
        &self,
        _request: &SessionRuntimeBuildInput,
        _existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        let model = self.model.read().await;
        Ok(ResolvedSessionRuntime {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId("main".to_string()),
                model: model.name.clone(),
                llm: None,
                system_prompt:
                    "You are xiaoO in a text conversation. Respond to the user directly."
                        .to_string(),
                feature_flags: FeatureFlags {
                    tool_execution: false,
                    skill_matching: false,
                    skill_discovery: false,
                    kvcache_debug_enabled: false,
                    ..Default::default()
                },
                token_budget: TokenBudgetConfig {
                    total_budget: model.context_window,
                    reserved_for_output: 2048,
                    reserved_for_system: 512,
                    hard_limit_ratio: 0.9,
                },
                workspace_root: self.cwd.clone(),
                max_turns: None,
                subagent_roles: Default::default(),
            },
            entry_kind: None,
            llm_provider: model.provider.clone(),
            tool_registry: Some(Arc::new(EmptyToolRegistry::new())),
            skill_registry: None,
            bindings: SessionRuntimeBindings::default(),
            compression_pipeline: None,
            trace: serde_json::json!({"storage_backend": "noop"}),
            hooker: no_hooks(),
            operation_backend: None,
            backend_workspace_root: self.cwd.clone(),
            e2b_bootstrap: None,
            bootstrap_binding: None,
            e2b_finalized: false,
        })
    }
}

pub struct ControlledTextSession {
    entry: SessionOpenRequest,
    resolver: Arc<ControlledResolver>,
    service: Arc<dyn SessionService>,
    control: Arc<dyn SessionControlPlane>,
    usage: Arc<Mutex<TurnUsage>>,
}

impl ControlledTextSession {
    pub async fn open(
        session_id: String,
        cwd: PathBuf,
        profile: &ControlledModelProfile,
    ) -> anyhow::Result<Self> {
        let backend_manager = Arc::new(BackendManager::new_controlled());
        let workspace = cwd
            .to_str()
            .filter(|_| cwd.is_absolute())
            .ok_or_else(|| anyhow::anyhow!("cwd must be an absolute UTF-8 path"))?;
        let root = BackendPath::from_raw(workspace);
        let now = agent_types::current_time_ms();
        let instance = BackendInstance {
            backend_id: BackendId(session_id.clone()),
            provider: BackendProviderKind("acp".to_string()),
            instance_id: BackendInstanceId(session_id.clone()),
            session_id: session_id.clone(),
            state: BackendLifecycleState::Active,
            workspace_root: root.clone(),
            endpoint: None,
            snapshot: None,
            capabilities: Default::default(),
            resources: Default::default(),
            metadata: serde_json::Value::Null,
            created_at_ms: now,
            updated_at_ms: now,
        };
        backend_manager
            .bind_controlled(
                instance,
                Arc::new(TextOnlyBackend {
                    session_id: session_id.clone(),
                    root,
                }),
            )
            .await;
        Self::open_with_backend_manager(session_id, cwd, profile, backend_manager).await
    }

    async fn open_with_backend_manager(
        session_id: String,
        cwd: PathBuf,
        profile: &ControlledModelProfile,
        backend_manager: Arc<BackendManager>,
    ) -> anyhow::Result<Self> {
        let usage = Arc::new(Mutex::new(TurnUsage::default()));
        let model = profile.resolve(usage.clone()).await?;
        let resolver = Arc::new(ControlledResolver {
            model: RwLock::new(model),
            cwd: cwd.clone(),
        });
        let entry = SessionOpenRequest {
            session_id: session_id.clone(),
            conversation_id: session_id.clone(),
            sender_id: "acp".to_string(),
            entry: GatewayEntryContext::default(),
            channel: None,
            channel_instance_id: None,
            llm: None,
            workspace: Some(cwd.clone()),
            skills: None,
            client_id: Some("stdio".to_string()),
            client_pid: None,
            client_hostname: None,
        };
        let app = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            Arc::new(InMemorySessionStore::default()),
            resolver.clone(),
            no_hooks(),
            backend_manager,
        )?;
        app.session_control_plane
            .open_session(entry.clone())
            .await?;
        Ok(Self {
            entry,
            resolver,
            service: app.session_service,
            control: app.session_control_plane,
            usage,
        })
    }

    pub async fn select_model(&self, profile: &ControlledModelProfile) -> anyhow::Result<()> {
        let model = profile.resolve(self.usage.clone()).await?;
        *self.resolver.model.write().await = model;
        Ok(())
    }

    pub async fn prompt(
        &self,
        text: String,
        events: Arc<dyn LoopEventSink>,
        cancel: CancellationToken,
    ) -> (
        Result<TurnOutcome, super::SessionServiceError>,
        ControlledTurnUsage,
    ) {
        *self.usage.lock().unwrap_or_else(|error| error.into_inner()) = TurnUsage::default();
        let result = self
            .service
            .run_turn_with_interaction(
                self.entry.clone().into_turn_request(text),
                Some(events),
                None,
                None,
                Some(cancel),
                None,
            )
            .await
            .map(|turn| turn.outcome);
        let usage = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        let status = match (
            usage.reported,
            usage.incomplete || result.is_err() || matches!(result, Ok(TurnOutcome::Cancelled)),
        ) {
            (false, _) => "unavailable",
            (true, true) => "partial",
            (true, false) => "complete",
        };
        let usage = ControlledTurnUsage {
            version: 1,
            input_tokens: usage.reported.then_some(usage.input),
            output_tokens: usage.reported.then_some(usage.output),
            status,
        };
        (result, usage)
    }

    pub async fn close(&self) -> Result<(), super::SessionServiceError> {
        self.control
            .force_close_session(&self.entry.session_id)
            .await
            .map(|_| ())
    }
}

struct TextOnlyBackend {
    session_id: String,
    root: BackendPath,
}

fn text_only() -> OperationError {
    OperationError::Unsupported {
        message: "workspace operations are disabled in text sessions".to_string(),
    }
}

#[async_trait]
impl OperationBackend for TextOnlyBackend {
    fn backend_id(&self) -> &str {
        &self.session_id
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        OperationBackendCapabilities {
            supports_atomic_write: false,
            supports_grep: false,
            supports_export_file: false,
            supports_lsp: false,
        }
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        self
    }
    fn files(&self) -> &dyn OperationFileSystem {
        self
    }
    fn search(&self) -> &dyn OperationSearch {
        self
    }
    fn exec(&self) -> &dyn OperationExec {
        self
    }
    fn export(&self) -> &dyn OperationExport {
        self
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        Ok(())
    }
}

#[async_trait]
impl OperationPathResolver for TextOnlyBackend {
    fn workspace_root(&self) -> &BackendPath {
        &self.root
    }
    fn home_dir(&self) -> Option<&BackendPath> {
        None
    }
    async fn resolve_path(&self, _: ResolvePathRequest) -> Result<BackendPath, OperationError> {
        Err(text_only())
    }
}

#[async_trait]
impl OperationFileSystem for TextOnlyBackend {
    async fn stat(&self, _: &BackendPath) -> Result<PathStat, OperationError> {
        Err(text_only())
    }
    async fn read_bytes(&self, _: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        Err(text_only())
    }
    async fn write_bytes(&self, _: WriteBytesRequest) -> Result<WriteBytesOutcome, OperationError> {
        Err(text_only())
    }
    async fn create_dir_all(&self, _: &BackendPath) -> Result<(), OperationError> {
        Err(text_only())
    }
    async fn temp_path(&self, _: TempPathRequest) -> Result<BackendPath, OperationError> {
        Err(text_only())
    }
}

#[async_trait]
impl OperationSearch for TextOnlyBackend {
    async fn glob(&self, _: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
        Err(text_only())
    }
    async fn grep(&self, _: GrepRequest) -> Result<GrepResult, OperationError> {
        Err(text_only())
    }
}

#[async_trait]
impl OperationExec for TextOnlyBackend {
    async fn exec(&self, _: ExecRequest) -> Result<ExecResult, OperationError> {
        Err(text_only())
    }
}

#[async_trait]
impl OperationExport for TextOnlyBackend {
    async fn export_file(
        &self,
        _: ExportFileRequest,
    ) -> Result<SharedExportedFileHandle, OperationError> {
        Err(text_only())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/shared/gateway/controlled_session_test.rs"]
mod tests;
