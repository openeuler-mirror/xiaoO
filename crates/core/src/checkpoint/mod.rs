//! Context checkpoints captured and restored at agent-loop boundaries.
//!
//! `init` acquires access to a session's checkpoint backend; `release` relinquishes
//! it without destroying the agent-owned driver. The first init returns that
//! driver for attachment; subsequent init calls return only binding information.
//! Both lifecycle transitions are queued alongside save/restore/delete.
//! Release closes API acceptance immediately, but prior accepted commands run
//! before the driver cools. Re-init queues activation on the same driver, so a
//! delayed Release cannot undo a later activation. MemoryStorage snapshots remain
//! available across these cycles; only explicit deletion removes them.
//! Driver destruction is reserved for agent teardown, not handle release.
mod backend;
mod capture;
mod config;
mod contract;
mod driver;
mod factory;
#[cfg(unix)]
mod session;
#[cfg(unix)]
mod socket;
mod state;
mod storage;
mod types;

pub use config::{CheckpointConfig, CheckpointTransport, SaveMode, UnixSocketConfig};
pub use contract::{CheckpointBackend, CheckpointUpdateSink};
pub use driver::CheckpointDriver;
pub use factory::create;
#[cfg(unix)]
pub use session::UnixCheckpointSession;
#[cfg(unix)]
pub use socket::UnixCheckpointEndpoint;
pub use storage::{
    MemoryCachedSqliteStorageConfig, MemoryStorageConfig, StorageConfig, StorageKind,
};
pub use tool::CheckpointTodoItem;
pub use types::{
    AccessMode, BindingInfo, CheckpointBinding, CheckpointError, RestoreMode, RestoreRequest,
    Snapshot, SnapshotContext, SnapshotId,
};

#[cfg(test)]
#[path = "../../../../tests/unit/core/checkpoint_test.rs"]
mod tests;
