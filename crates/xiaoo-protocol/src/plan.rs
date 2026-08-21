//! Plan snapshot types shared across the wire boundary.
//!
//! [`TodoDisplayStatus`] and [`TodoSnapshotItem`] are the plan-panel payload
//! carried inside SSE `PlanUpdate` events.  Their serde representation is a
//! wire contract: any field change must stay byte-for-byte compatible with
//! the daemon's serialization.  The definitions live here (in the protocol
//! crate) so both the SSE event model and downstream consumers reference a
//! single source; `xiaoo_shared::plan` re-exports them to preserve existing
//! import paths.

use serde::{Deserialize, Serialize};

/// Display status of a single todo item in the TUI's plan panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoDisplayStatus {
    /// Not yet started.
    Pending,
    /// Currently being worked on.
    InProgress,
    /// Finished.
    Completed,
}

/// One row in the TUI plan panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoSnapshotItem {
    /// Display status of the row.
    pub status: TodoDisplayStatus,
    /// Text content of the row.
    pub content: String,
}
