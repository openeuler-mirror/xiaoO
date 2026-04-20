use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    #[allow(dead_code)]
    pub fn remove_read_state(&mut self, path: &str) -> Option<FileReadState> {
        self.states.remove(path)
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.states.clear();
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

impl Default for FileReadState {
    fn default() -> Self {
        Self {
            timestamp: 0,
            offset: None,
            limit: None,
            is_partial_view: false,
        }
    }
}

#[allow(dead_code)]
pub fn get_file_mtime(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
}

pub fn system_time_to_timestamp(time: Option<SystemTime>) -> i64 {
    time.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
