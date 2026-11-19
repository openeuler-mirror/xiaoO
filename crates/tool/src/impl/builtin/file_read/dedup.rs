use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileReadState {
    pub timestamp: i64,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    pub is_partial_view: bool,
}

#[derive(Debug)]
struct RecordedRead {
    state: FileReadState,
    complete: bool,
}

#[derive(Debug, Default)]
pub struct DedupStateStore {
    states: HashMap<String, RecordedRead>,
}

impl DedupStateStore {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    pub fn get_read_state(&self, path: &str) -> Option<FileReadState> {
        self.states.get(path).map(|read| read.state.clone())
    }

    pub fn set_read_state(&mut self, path: String, state: FileReadState) {
        self.states.insert(
            path,
            RecordedRead {
                state,
                complete: true,
            },
        );
    }

    pub(crate) fn set_preview_state(&mut self, path: String, state: FileReadState) {
        // Keep the freshness check for an edit while allowing the read to be retried.
        self.states.insert(
            path,
            RecordedRead {
                state,
                complete: false,
            },
        );
    }

    pub fn is_file_unchanged(
        &self,
        path: &str,
        current_mtime: i64,
        offset: Option<u64>,
        limit: Option<u64>,
        is_partial_view: bool,
    ) -> bool {
        self.states.get(path).is_some_and(|read| {
            read.complete
                && read.state.timestamp == current_mtime
                && read.state.offset == offset
                && read.state.limit == limit
                && read.state.is_partial_view == is_partial_view
        })
    }
}

// Millisecond resolution, not seconds: dedup suppresses *full* re-reads as
// well as paged ones, so a re-read after an edit that landed in the same
// wall-clock second as the prior read must still be seen as changed —
// otherwise the agent gets a stale "File is unchanged" and never sees the
// new content.
pub fn system_time_to_timestamp(time: Option<SystemTime>) -> i64 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
