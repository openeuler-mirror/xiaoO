use std::sync::Arc;

use agent_contracts::backend::{
    capability::{
        filesystem::{ReadBytesRequest, TempPathRequest, WriteBytesOutcome, WriteBytesRequest},
        OperationExec, OperationExport, OperationFileSystem, OperationPathResolver,
        OperationSearch,
    },
    BackendPath, OperationBackend, OperationBackendCapabilities, OperationError,
    OperationPermissionControl, PathStat,
};
use agent_contracts::{
    AgentContext, HookerRegistry, InteractionHandle, RuntimeView, ToolEventSink, ToolStateStore,
    TraceRecorder,
};

pub(crate) struct BackendTestRuntime(Arc<dyn OperationBackend>);

impl BackendTestRuntime {
    pub(crate) fn new(backend: Arc<dyn OperationBackend>) -> Self {
        Self(backend)
    }
}

impl RuntimeView for BackendTestRuntime {
    fn state_store(&self) -> &dyn ToolStateStore {
        panic!("not used in file tool tests")
    }

    fn tool_events(&self) -> &dyn ToolEventSink {
        panic!("not used in file tool tests")
    }

    fn trace_recorder(&self) -> &dyn TraceRecorder {
        panic!("not used in file tool tests")
    }

    fn agent_context(&self) -> &dyn AgentContext {
        panic!("not used in file tool tests")
    }

    fn interaction(&self) -> &dyn InteractionHandle {
        panic!("not used in file tool tests")
    }

    fn hookers(&self) -> &dyn HookerRegistry {
        panic!("not used in file tool tests")
    }

    fn operation_backend(&self) -> Option<Arc<dyn OperationBackend>> {
        Some(Arc::clone(&self.0))
    }
}

struct AtomicWriteCapabilityOverride {
    inner: Arc<dyn OperationBackend>,
    files: TestFileSystem,
    supports_atomic_write: bool,
}

struct TestFileSystem {
    inner: Arc<dyn OperationBackend>,
}

pub(crate) fn override_atomic_write_capability(
    inner: Arc<dyn OperationBackend>,
    supports_atomic_write: bool,
) -> Arc<dyn OperationBackend> {
    Arc::new(AtomicWriteCapabilityOverride {
        files: TestFileSystem {
            inner: Arc::clone(&inner),
        },
        inner,
        supports_atomic_write,
    })
}

#[async_trait::async_trait]
impl OperationFileSystem for TestFileSystem {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        self.inner.files().stat(path).await
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        self.inner.files().read_bytes(request).await
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        self.inner.files().write_bytes(request).await
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        self.inner.files().create_dir_all(path).await
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        self.inner.files().temp_path(request).await
    }
}

#[async_trait::async_trait]
impl OperationBackend for AtomicWriteCapabilityOverride {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        let mut capabilities = self.inner.capabilities();
        capabilities.supports_atomic_write = self.supports_atomic_write;
        capabilities
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        self.inner.paths()
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files
    }

    fn search(&self) -> &dyn OperationSearch {
        self.inner.search()
    }

    fn exec(&self) -> &dyn OperationExec {
        self.inner.exec()
    }

    fn export(&self) -> &dyn OperationExport {
        self.inner.export()
    }

    fn attach_interaction(&self, interaction: Arc<dyn InteractionHandle>) {
        self.inner.attach_interaction(interaction);
    }

    fn permission_control(&self) -> Option<&dyn OperationPermissionControl> {
        self.inner.permission_control()
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        self.inner.shutdown().await
    }
}

/// A mutation observed by [`record_extra_forwarding`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RecordedMutation {
    WriteBytes {
        path: String,
        extra: Option<serde_json::Value>,
    },
    CreateDirAll {
        path: String,
        extra: Option<serde_json::Value>,
    },
}

struct ExtraRecordingBackend {
    inner: Arc<dyn OperationBackend>,
    files: ExtraRecordingFileSystem,
}

struct ExtraRecordingFileSystem {
    inner: Arc<dyn OperationBackend>,
    recorded: std::sync::Arc<std::sync::Mutex<Vec<RecordedMutation>>>,
}

/// Wrap `inner` so every mutation-class filesystem request records its
/// `extra` channel (and path) for later inspection, then delegates to the
/// real backend. Used to assert the file executors forward `FinalToolCall
/// .extra` into `WriteBytesRequest.extra` / `create_dir_all_with_extra`.
pub(crate) fn record_extra_forwarding(
    inner: Arc<dyn OperationBackend>,
) -> (
    Arc<dyn OperationBackend>,
    std::sync::Arc<std::sync::Mutex<Vec<RecordedMutation>>>,
) {
    let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    (
        Arc::new(ExtraRecordingBackend {
            files: ExtraRecordingFileSystem {
                inner: Arc::clone(&inner),
                recorded: Arc::clone(&recorded),
            },
            inner,
        }),
        recorded,
    )
}

#[async_trait::async_trait]
impl OperationFileSystem for ExtraRecordingFileSystem {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        self.inner.files().stat(path).await
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        self.inner.files().read_bytes(request).await
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        self.recorded
            .lock()
            .unwrap()
            .push(RecordedMutation::WriteBytes {
                path: request.path.native().to_string(),
                extra: request.extra.clone(),
            });
        self.inner.files().write_bytes(request).await
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        self.recorded
            .lock()
            .unwrap()
            .push(RecordedMutation::CreateDirAll {
                path: path.native().to_string(),
                extra: None,
            });
        self.inner.files().create_dir_all(path).await
    }

    async fn create_dir_all_with_extra(
        &self,
        path: &BackendPath,
        extra: Option<&serde_json::Value>,
    ) -> Result<(), OperationError> {
        self.recorded
            .lock()
            .unwrap()
            .push(RecordedMutation::CreateDirAll {
                path: path.native().to_string(),
                extra: extra.cloned(),
            });
        self.inner
            .files()
            .create_dir_all_with_extra(path, extra)
            .await
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        self.inner.files().temp_path(request).await
    }
}

#[async_trait::async_trait]
impl OperationBackend for ExtraRecordingBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        self.inner.capabilities()
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        self.inner.paths()
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files
    }

    fn search(&self) -> &dyn OperationSearch {
        self.inner.search()
    }

    fn exec(&self) -> &dyn OperationExec {
        self.inner.exec()
    }

    fn export(&self) -> &dyn OperationExport {
        self.inner.export()
    }

    fn attach_interaction(&self, interaction: Arc<dyn InteractionHandle>) {
        self.inner.attach_interaction(interaction);
    }

    fn permission_control(&self) -> Option<&dyn OperationPermissionControl> {
        self.inner.permission_control()
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        self.inner.shutdown().await
    }
}
