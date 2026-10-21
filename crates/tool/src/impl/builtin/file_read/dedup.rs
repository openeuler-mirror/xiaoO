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

#[derive(Debug, Default)]
pub struct DedupStateStore {
    states: HashMap<String, FileReadState>,
}

impl DedupStateStore {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    pub fn get_read_state(&self, path: &str) -> Option<FileReadState> {
        self.states.get(path).cloned()
    }

    pub fn set_read_state(&mut self, path: String, state: FileReadState) {
        self.states.insert(path, state);
    }

    pub fn is_file_unchanged(
        &self,
        path: &str,
        current_mtime: i64,
        offset: Option<u64>,
        limit: Option<u64>,
        is_partial_view: bool,
    ) -> bool {
        match self.get_read_state(path) {
            Some(state) => {
                state.timestamp == current_mtime
                    && state.offset == offset
                    && state.limit == limit
                    && state.is_partial_view == is_partial_view
            }
            None => false,
        }
    }
}

// Millisecond resolution, not seconds: dedup now suppresses *full* re-reads (not
// just paged ones), so a re-read after an edit that landed in the same wall-clock
// second as the prior read must still be seen as changed — otherwise the agent
// gets a stale "File is unchanged" and never sees the new content.
pub fn system_time_to_timestamp(time: Option<SystemTime>) -> i64 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
