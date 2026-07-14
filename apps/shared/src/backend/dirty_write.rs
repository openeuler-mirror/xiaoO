use agent_contracts::backend::capability::exec::{ExecRequest, ExecResult, OperationExec};
use agent_contracts::backend::capability::filesystem::{
    OperationFileSystem, ReadBytesRequest, TempPathRequest, WriteBytesOutcome, WriteBytesRequest,
};
use agent_contracts::backend::capability::path::OperationPathResolver;
use agent_contracts::backend::capability::search::OperationSearch;
use agent_contracts::backend::{
    BackendPath, OperationBackend, OperationBackendCapabilities, OperationError,
    OperationPermissionControl, PathStat,
};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use super::BackendCheckpointRef;

#[derive(Default)]
pub(super) struct BackendDirtyTracker {
    state: Mutex<BackendDirtyState>,
}

#[derive(Default)]
struct BackendDirtyState {
    dirty: bool,
    checkpoint: Option<BackendCheckpointRef>,
}

impl BackendDirtyTracker {
    pub(super) fn mark_dirty(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.dirty = true;
        }
    }

    pub(super) fn is_dirty(&self) -> bool {
        self.state.lock().map(|state| state.dirty).unwrap_or(true)
    }

    pub(super) fn checkpoint(&self) -> Option<BackendCheckpointRef> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.checkpoint.clone())
    }

    pub(super) fn set_checkpoint(&self, checkpoint: BackendCheckpointRef) {
        if let Ok(mut state) = self.state.lock() {
            state.checkpoint = Some(checkpoint);
            state.dirty = false;
        }
    }

    pub(super) fn clear_checkpoint_if_matches(&self, checkpoint_id: &str) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.checkpoint_id == checkpoint_id)
            {
                state.checkpoint = None;
                state.dirty = true;
            }
        }
    }
}

pub(super) struct DirtyTrackedOperationBackend {
    inner: Arc<dyn OperationBackend>,
    files: DirtyTrackedFileSystem,
    exec: DirtyTrackedExec,
}

impl DirtyTrackedOperationBackend {
    pub(super) fn wrap(
        inner: Arc<dyn OperationBackend>,
        tracker: Arc<BackendDirtyTracker>,
    ) -> Arc<dyn OperationBackend> {
        Arc::new(Self {
            files: DirtyTrackedFileSystem {
                inner: Arc::clone(&inner),
                tracker: Arc::clone(&tracker),
            },
            exec: DirtyTrackedExec {
                inner: Arc::clone(&inner),
                tracker,
            },
            inner,
        })
    }
}

#[async_trait]
impl OperationBackend for DirtyTrackedOperationBackend {
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
        &self.exec
    }

    fn export(&self) -> &dyn agent_contracts::backend::capability::export::OperationExport {
        self.inner.export()
    }

    fn permission_control(&self) -> Option<&dyn OperationPermissionControl> {
        self.inner.permission_control()
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        self.inner.shutdown().await
    }
}

struct DirtyTrackedFileSystem {
    inner: Arc<dyn OperationBackend>,
    tracker: Arc<BackendDirtyTracker>,
}

#[async_trait]
impl OperationFileSystem for DirtyTrackedFileSystem {
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
        let outcome = self.inner.files().write_bytes(request).await?;
        self.tracker.mark_dirty();
        Ok(outcome)
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        self.inner.files().create_dir_all(path).await?;
        self.tracker.mark_dirty();
        Ok(())
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        let path = self.inner.files().temp_path(request).await?;
        self.tracker.mark_dirty();
        Ok(path)
    }
}

struct DirtyTrackedExec {
    inner: Arc<dyn OperationBackend>,
    tracker: Arc<BackendDirtyTracker>,
}

#[async_trait]
impl OperationExec for DirtyTrackedExec {
    fn default_shell(&self) -> Option<&str> {
        self.inner.exec().default_shell()
    }

    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        let result = self.inner.exec().exec(request).await?;
        self.tracker.mark_dirty();
        Ok(result)
    }
}
