//! File read deduplication logic.
//!
//! Tracks file read state to detect when a file has been read without changes.
//! Uses mtime (modification time) to detect file changes.

use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

/// State tracked for a previously read file.
/// Used to detect if a file has changed since last read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileReadState {
    /// Last known modification time of the file (as UNIX timestamp).
    pub timestamp: i64,
    /// The line number where reading started (1-indexed).
    pub offset: Option<u64>,
    /// The number of lines that were read.
    pub limit: Option<u64>,
    /// Whether this was a partial view of the file.
    pub is_partial_view: bool,
}

/// In-memory store for file read dedup states.
/// Maps file path -> FileReadState.
#[derive(Debug, Default)]
pub struct DedupStateStore {
    states: HashMap<String, FileReadState>,
}

impl DedupStateStore {
    /// Creates a new empty dedup state store.
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Gets the read state for a file path.
    pub fn get_read_state(&self, path: &str) -> Option<FileReadState> {
        self.states.get(path).cloned()
    }

    /// Sets the read state for a file path.
    pub fn set_read_state(&mut self, path: String, state: FileReadState) {
        self.states.insert(path, state);
    }

    /// Removes the read state for a file path.
    #[allow(dead_code)]
    pub fn remove_read_state(&mut self, path: &str) -> Option<FileReadState> {
        self.states.remove(path)
    }

    /// Clears all stored states.
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Checks if a file can be considered unchanged since last read.
    /// Returns true if the file was previously read with the same parameters
    /// and the mtime hasn't changed.
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

/// Gets the modification time of a file as a UNIX timestamp.
/// Returns None if the file doesn't exist or cannot be accessed.
#[allow(dead_code)]
pub fn get_file_mtime(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs() as i64)
}
