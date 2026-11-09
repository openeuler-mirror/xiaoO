use super::{
    AccessMode, CheckpointBinding, CheckpointError, RestoreRequest, Snapshot, SnapshotContext,
    SnapshotId,
};
use async_trait::async_trait;

#[async_trait]
pub trait CheckpointBackend: Send + Sync {
    /// Parse a JSON config object, resolve overrides against startup defaults,
    /// and assemble the capture strategy. An empty object uses startup defaults.
    /// Acquire use of this session's checkpoint backend. Activation is queued.
    /// Re-init after release resolves options afresh; it does not inherit overrides.
    async fn init(
        &self,
        context: SnapshotContext,
        config: serde_json::Value,
    ) -> Result<CheckpointBinding, CheckpointError>;
    fn save(&self, context: SnapshotContext) -> Result<Snapshot, CheckpointError>;
    /// List Pending/Ready IDs, newest save first. None means 10; negative means all.
    fn list_checkpoint(
        &self,
        context: SnapshotContext,
        access_mode: AccessMode,
        count: Option<i64>,
    ) -> Result<Vec<SnapshotId>, CheckpointError>;
    /// EnsureSafeMode accepts Ready; PromiseMode accepts Pending or Ready.
    async fn restore(
        &self,
        context: SnapshotContext,
        request: RestoreRequest,
    ) -> Result<(), CheckpointError>;
    async fn delete_snapshot(
        &self,
        context: SnapshotContext,
        id: &SnapshotId,
    ) -> Result<(), CheckpointError>;
    /// Reject new operations immediately; queue cooling after accepted commands.
    /// The agent-owned driver and saved snapshots survive release.
    /// A different session context has no effect.
    fn release(&self, context: SnapshotContext);
}

/// Receives successful restores. Implementations should enqueue UI/application
/// updates without blocking; checkpoint itself does not depend on a UI.
pub trait CheckpointUpdateSink: Send + Sync {
    fn on_restore(
        &self,
        session_id: &str,
        messages: &[agent_types::ChatMessage],
        todos: &[super::CheckpointTodoItem],
    );
}
