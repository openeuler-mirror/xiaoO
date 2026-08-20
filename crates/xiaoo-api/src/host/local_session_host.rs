//! `LocalSessionHost` / `LocalSessionHostBuilder` / `SecretsInit` / `HostBuildError`。
//!
//! 【L0 门面层】host 持有进程级资源（会话存储、backend 管理器、hooker 注册表、
//! 记忆自动化连接、密钥 provider），并暴露 `open_session` / `shutdown` 主入口。
//! 一个进程通常只建一个 host；多会话共享同一 host。
//!
//! 提炼纪律：逐行对照门面 → 现有实现的映射表，**不新增行为**。

use std::sync::Arc;
use std::time::Duration;

use agent_types::hook::HookerRegistryConfig;
use mcp::McpServerConfig;
use thiserror::Error;
use tokio::sync::watch;
use tracing::warn;

use xiaoo_shared::backend::BackendManager;
use xiaoo_shared::gateway::bootstrap::{AppBootstrap, AppBootstrapError, AppDependencies};
use xiaoo_shared::gateway::memory_automation::{
    McpMemoryAutomation, MemoryAutomationConfig, MemoryAutomationHealth, TurnMemoryAutomation,
};
use xiaoo_shared::gateway::{
    HostedSessionRuntimeConfig, SessionControlPlane, SessionOpenRequest, SessionServiceError,
    SessionStore,
};

use super::options::SessionOptions;

/// 密钥 provider 初始化方式。
///
/// - `OnDemand`（缺省）：不显式初始化。运行期 [`crate::secrets::get_decrypted_api_key`]
///   返回 `None`（无 provider），派生自动回退到 provider 惯例环境变量。
///   适用于进程未配置加密密钥、直接读环境变量的场景。
/// - `WithProvider { path, use_sdf }`：调用
///   [`crate::secrets::init_secret_provider`] 初始化全局 provider，
///   `path` 指向 `llm_secrets.json`。SDF/TEE 场景设 `use_sdf = true`。
/// - `Inherit`：进程已自行初始化（或测试替身），host 不再初始化。行为同 `OnDemand`，
///   但语义上声明"已初始化"，便于调用方文档化其约定。
///
/// **设计取舍**：`OnDemand` 不持有 `config_path` 字段，因此无法直接定位
/// `llm_secrets.json`。本实现选择"不显式初始化"作为缺省语义——这是最不会
/// 出错的默认（不会因为没有配置文件而报错），调用方需要密钥加密时改用
/// `WithProvider { path, use_sdf }`。
#[derive(Debug, Clone)]
pub enum SecretsInit {
    /// 不显式初始化（缺省）。`get_decrypted_api_key` 返回 `None`，
    /// 派生回退到 provider 惯例环境变量。
    OnDemand,
    /// 显式初始化：`path` 指向 `llm_secrets.json`；`use_sdf` 启用 SDF/TEE。
    WithProvider {
        /// `llm_secrets.json` 的路径。
        path: std::path::PathBuf,
        /// 是否启用 SDF/TEE 解密。
        use_sdf: bool,
    },
    /// 进程已自行初始化，host 不再初始化。
    Inherit,
}

impl Default for SecretsInit {
    fn default() -> Self {
        Self::OnDemand
    }
}

/// `LocalSessionHostBuilder::build` 的失败原因。
///
/// 本类型为 host 构建阶段的独立错误；派生类失败折叠进
/// `SessionServiceError`。`LocalSessionHost::open_session` / `Session::send` 等
/// 后续操作的失败统一用 `SessionServiceError` 表达。
#[derive(Debug, Error)]
pub enum HostBuildError {
    /// `AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation`
    /// 失败（通常是 hooker 注册表构建失败）。
    #[error("bootstrap failed: {0}")]
    Bootstrap(#[from] AppBootstrapError),
    /// `init_secret_provider` 或前置的密钥文件读取失败。
    #[error("secret provider init failed: {0}")]
    Secrets(String),
    /// `McpMemoryAutomation::connect` 失败。
    #[error("memory automation connect failed: {0}")]
    Memory(String),
}

/// 进程级资源容器：会话存储、backend 管理器、hooker 注册表、记忆自动化连接、
/// 密钥 provider。
///
/// 一个进程通常只建一个 [`LocalSessionHost`]；多会话共享同一 host。
/// `shutdown` 后既有 `Session` 句柄的调用返回 `SessionServiceError`（会话不存在）。
///
/// 提炼自 endside 现有代码：`cli/entry.rs:1053-1077`、`session_core.rs:33-71`、
/// `session_core.rs:315-349`、`session_core.rs:73-146`（逐行对照）。
pub struct LocalSessionHost {
    pub(crate) inner: Arc<HostInner>,
}

pub(crate) struct HostInner {
    pub(crate) session_store: Arc<dyn SessionStore>,
    pub(crate) backend_manager: Arc<BackendManager>,
    pub(crate) memory_automation: Option<Arc<dyn TurnMemoryAutomation>>,
    pub(crate) hooker_config: HookerRegistryConfig,
    pub(crate) interaction_timeout: Option<Duration>,
    /// 用于 `force_close_session`（生命周期管理）。`open_session` 走 per-open bootstrap，
    /// 但 close 走 lifecycle control plane（NoopResolver 即可，close 不需要 resolve）。
    pub(crate) lifecycle_control_plane: Arc<dyn SessionControlPlane>,
    /// 记忆自动化健康订阅，供 `memory_health()` 透出。
    pub(crate) memory_health: Option<watch::Receiver<MemoryAutomationHealth>>,
    /// 跟踪本 host 打开的会话 id，供 `shutdown` 时 force_close。
    pub(crate) active_session_ids: tokio::sync::Mutex<Vec<String>>,
    /// 后台任务句柄：持有以保活（drop 不 abort，但保留 strong ref 防止意外释放）。
    _reaper_handle: Option<tokio::task::JoinHandle<()>>,
    _signal_handle: Option<tokio::task::JoinHandle<()>>,
}

impl LocalSessionHost {
    /// 构造 builder。
    pub fn builder() -> LocalSessionHostBuilder {
        LocalSessionHostBuilder::default()
    }

    /// 打开（或按 `options.session_id` 幂等复用 / 恢复）会话，返回会话句柄。
    ///
    /// 内部：`SessionOptions` 派生 → `HostedSessionRuntimeResolver` 构造
    /// （per-open，empty bindings）→ `AppBootstrap` → `control_plane.open_session`
    /// （对照 endside `session_core.rs:49-71`、`297-313`）。返回的 `Session` 缓存
    /// `(open_request, runtime_config)` 供后续 turn 使用。
    pub async fn open_session(
        &self,
        options: SessionOptions,
    ) -> Result<super::session::Session, SessionServiceError> {
        let (open_request, runtime_config) = options.derive().await?;
        self.open_session_with(open_request, runtime_config).await
    }

    /// 用手工构造的请求与 runtime 配置开会话（跳过 `SessionOptions` 派生）。
    /// 供 advanced 装配与测试替身使用。
    pub async fn open_session_with(
        &self,
        request: SessionOpenRequest,
        config: HostedSessionRuntimeConfig,
    ) -> Result<super::session::Session, SessionServiceError> {
        let session_id = request.session_id.clone();

        // per-open bootstrap（对照 endside session_core.rs:297-311）
        let hooker_config = self.inner.hooker_config.clone();
        let resolver = Arc::new(xiaoo_shared::gateway::HostedSessionRuntimeResolver::new(
            config.clone(),
            xiaoo_shared::gateway::SessionRuntimeBindings::default(),
        ));
        let deps = AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
            self.inner.session_store.clone(),
            resolver,
            hooker_config,
            self.inner.backend_manager.clone(),
            self.inner.memory_automation.clone(),
            self.inner.interaction_timeout,
        )
        .map_err(|e| SessionServiceError::RuntimeBuild {
            message: e.to_string(),
        })?;

        deps.session_control_plane
            .open_session(request.clone())
            .await?;

        self.inner
            .active_session_ids
            .lock()
            .await
            .push(session_id.clone());

        Ok(super::session::Session::new(
            self.inner.clone(),
            session_id,
            request,
            config,
        ))
    }

    /// 关闭全部会话并释放资源：逐会话 `force_close` →
    /// `backend_manager.shutdown_all()`（限时 `backend_timeout`）→
    /// memory automation `close()`（限时 5s）。幂等。
    ///
    /// 对照 endside `session_core.rs:315-349` + serverside `main.rs:243-273`。
    pub async fn shutdown(&self, backend_timeout: Duration) {
        // 1. 逐会话 force_close
        let ids: Vec<String> = {
            let mut lock = self.inner.active_session_ids.lock().await;
            let ids = lock.drain(..).collect::<Vec<_>>();
            ids
        };
        for session_id in &ids {
            if let Err(err) = self
                .inner
                .lifecycle_control_plane
                .force_close_session(session_id)
                .await
            {
                warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to close session on shutdown"
                );
            }
        }

        // 2. backend_manager.shutdown_all()（限时）
        let backend_shutdown = async {
            let _ = self.inner.backend_manager.shutdown_all().await;
        };
        if tokio::time::timeout(backend_timeout, backend_shutdown).await.is_err() {
            warn!(
                ?backend_timeout,
                "backend_manager.shutdown_all() timed out during host shutdown"
            );
        }

        // 3. memory automation close（限时 5s，对照 endside session_core.rs:339-347）
        if let Some(automation) = self.inner.memory_automation.as_ref() {
            let close = automation.close();
            match tokio::time::timeout(Duration::from_secs(5), close).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!(error = %error, "failed to close MCP memory automation"),
                Err(_) => warn!("MCP memory automation close timed out after 5 seconds"),
            }
        }
    }

    /// 记忆自动化健康订阅（`None` 表示未启用）；供状态灯渲染。
    pub fn memory_health(&self) -> Option<watch::Receiver<MemoryAutomationHealth>> {
        self.inner.memory_health.clone()
    }

    // ---- Advanced 访问器（L2；深度集成/测试用）----
    //
    // 注：本组不提供 `service()` 访问器。host 级只经 `lifecycle_only` 持有 noop
    // `SessionService`（仅能 force_close，不能 run_turn——run_turn 走 per-turn
    // bootstrap）。需要直接触达 `SessionService` 的调用方用
    // `Session::run_turn_raw`（advanced 直连）。

    /// 共享会话存储。
    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        self.inner.session_store.clone()
    }

    /// 共享 backend 管理器。
    pub fn backend_manager(&self) -> Arc<BackendManager> {
        self.inner.backend_manager.clone()
    }

    /// 生命周期 control plane（`open_session` / `force_close_session`）。
    pub fn control_plane(&self) -> Arc<dyn SessionControlPlane> {
        self.inner.lifecycle_control_plane.clone()
    }
}

/// `LocalSessionHost` 的 builder。
///
/// 缺省：`secrets = OnDemand`、`session_store = InMemorySessionStore::default()`、
/// `backend_manager = BackendManager::new()`（无 signal handler）、
/// `memory_automation = None`、`interaction_timeout = None`、`hooker = Default::default()`。
#[derive(Default)]
pub struct LocalSessionHostBuilder {
    hooker_config: HookerRegistryConfig,
    secrets_init: SecretsInit,
    session_store: Option<Arc<dyn SessionStore>>,
    backend_manager: Option<Arc<BackendManager>>,
    backend_signal_handler: bool,
    memory_automation_config: Option<(MemoryAutomationConfig, Vec<McpServerConfig>)>,
    memory_automation: Option<Arc<dyn TurnMemoryAutomation>>,
    interaction_timeout: Option<Duration>,
}

impl LocalSessionHostBuilder {
    /// hooker 注册表配置（config.toml `[hooker]` 段）。缺省：空注册表（不挂任何 hooker）。
    pub fn hooker_config(mut self, config: HookerRegistryConfig) -> Self {
        self.hooker_config = config;
        self
    }

    /// 密钥 provider 初始化方式。缺省 `OnDemand`。
    pub fn secrets(mut self, init: SecretsInit) -> Self {
        self.secrets_init = init;
        self
    }

    /// 会话存储；缺省为新建 `InMemorySessionStore`。
    /// 传入已有 store 可实现跨 host 共享（多会话 TUI 场景）。
    pub fn session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// backend 管理器；缺省为 `BackendManager::new()`。
    /// `with_signal_handler = true` 时启动跨进程 SIGUSR1 驱逐协作
    /// （常驻进程如 TUI 需要；单次运行的 CLI 不需要，缺省 false）。
    pub fn backend_manager(
        mut self,
        manager: Arc<BackendManager>,
        with_signal_handler: bool,
    ) -> Self {
        self.backend_manager = Some(manager);
        self.backend_signal_handler = with_signal_handler;
        self
    }

    /// 记忆自动化：由配置连接（内部调用 `McpMemoryAutomation::connect`；
    /// config 未启用时自动跳过）。与 `memory_automation()` 二选一。缺省：不启用。
    pub fn memory_automation_config(
        mut self,
        config: MemoryAutomationConfig,
        mcp_servers: Vec<McpServerConfig>,
    ) -> Self {
        self.memory_automation_config = Some((config, mcp_servers));
        self
    }

    /// 直接注入已构建的记忆自动化（测试替身用）。与 `memory_automation_config()` 二选一。
    pub fn memory_automation(mut self, automation: Arc<dyn TurnMemoryAutomation>) -> Self {
        self.memory_automation = Some(automation);
        self
    }

    /// subagent 交互超时（`AppBootstrap` 最后一个参数）。缺省 `None`（用内核默认）。
    pub fn interaction_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.interaction_timeout = timeout;
        self
    }

    /// 组装：secrets 初始化 → 连接 memory automation →
    /// `AppBootstrap::lifecycle_only` 构造 lifecycle control plane → 保存 host。
    ///
    /// 对照 endside `cli/entry.rs:1053-1077` + `session_core.rs:33-47`，
    /// 与 serverside `main.rs:83-117` 同构。
    pub async fn build(self) -> Result<LocalSessionHost, HostBuildError> {
        // 1. secrets 初始化
        match &self.secrets_init {
            SecretsInit::OnDemand | SecretsInit::Inherit => { /* no-op */ }
            SecretsInit::WithProvider { path, use_sdf } => {
                xiaoo_shared::gateway::init_secret_provider(path.clone(), *use_sdf);
                tracing::info!(
                    ?path,
                    use_sdf,
                    "secret provider initialized (WithProvider)",
                );
            }
        }

        // 2. backend manager（缺省 BackendManager::new）
        let backend_manager = self
            .backend_manager
            .clone()
            .unwrap_or_else(|| Arc::new(BackendManager::new()));

        // 3. session store（缺省 InMemorySessionStore）
        let session_store: Arc<dyn SessionStore> = self
            .session_store
            .clone()
            .unwrap_or_else(|| Arc::new(xiaoo_shared::gateway::InMemorySessionStore::default()));

        // 4. signal handler（可选）
        let signal_handle = if self.backend_signal_handler {
            Some(backend_manager.clone().start_signal_handler(session_store.clone()))
        } else {
            None
        };

        // 5. memory automation：直接注入优先；否则按配置连接
        let (memory_automation, memory_health) = if let Some(automation) = self.memory_automation.clone() {
            let health = automation.subscribe_health();
            (Some(automation), health)
        } else if let Some((config, mcp_servers)) = self.memory_automation_config.clone() {
            match McpMemoryAutomation::connect(config, &mcp_servers).await {
                Ok(Some(automation)) => {
                    let health = automation.subscribe_health();
                    (Some(automation), health)
                }
                Ok(None) => (None, None),
                Err(error) => {
                    warn!(
                        error = %error,
                        "memory automation disabled after host build error",
                    );
                    return Err(HostBuildError::Memory(error.to_string()));
                }
            }
        } else {
            (None, None)
        };

        // 6. lifecycle control plane（NoopResolver，仅用于 force_close 等生命周期操作）
        let deps: AppDependencies = AppBootstrap::lifecycle_only(
            session_store.clone(),
            self.hooker_config.clone(),
            backend_manager.clone(),
        )?;
        let lifecycle_control_plane = deps.session_control_plane;
        // lifecycle 的 reaper_handle 保活（drop 不 abort，但持有 strong ref 防止意外释放）
        let reaper_handle = deps.reaper_handle;

        Ok(LocalSessionHost {
            inner: Arc::new(HostInner {
                session_store,
                backend_manager,
                memory_automation,
                hooker_config: self.hooker_config.clone(),
                interaction_timeout: self.interaction_timeout,
                lifecycle_control_plane,
                memory_health,
                active_session_ids: tokio::sync::Mutex::new(Vec::new()),
                _reaper_handle: reaper_handle,
                _signal_handle: signal_handle,
            }),
        })
    }
}
