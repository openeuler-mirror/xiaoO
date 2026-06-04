//! Serializable file access audit records collected during execution.

use super::{FileAccessEvent, FileAccessResult, FileOperation};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A single file access event collected during one Cerberus execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAccessRecord {
    pub path: String,
    pub operation: FileAccessOperation,
    pub result: FileAccessOutcome,
    pub pid: u32,
    pub timestamp_nanos: u64,
}

/// File operation recorded in audit output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccessOperation {
    Read,
    Write,
    Execute,
}

/// Outcome of a file access attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAccessOutcome {
    Allowed,
    DeniedByLandlock,
    DeniedByPathNotFound,
}

impl From<FileAccessEvent> for FileAccessRecord {
    fn from(event: FileAccessEvent) -> Self {
        Self {
            path: event.path.display().to_string(),
            operation: event.operation.into(),
            result: event.result.into(),
            pid: event.pid,
            timestamp_nanos: system_time_to_nanos(event.timestamp),
        }
    }
}

impl From<FileOperation> for FileAccessOperation {
    fn from(operation: FileOperation) -> Self {
        match operation {
            FileOperation::Read => Self::Read,
            FileOperation::Write => Self::Write,
            FileOperation::Execute => Self::Execute,
        }
    }
}

impl From<FileAccessResult> for FileAccessOutcome {
    fn from(result: FileAccessResult) -> Self {
        match result {
            FileAccessResult::Allowed => Self::Allowed,
            FileAccessResult::DeniedByLandlock => Self::DeniedByLandlock,
            FileAccessResult::DeniedByPathNotFound => Self::DeniedByPathNotFound,
        }
    }
}

fn system_time_to_nanos(timestamp: SystemTime) -> u64 {
    timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

/// In-memory collector for file access events during one execution.
#[derive(Debug, Default)]
pub struct FileAccessCollector {
    events: Mutex<Vec<FileAccessRecord>>,
}

impl FileAccessCollector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, event: FileAccessEvent) {
        self.events
            .lock()
            .expect("file access collector mutex poisoned")
            .push(FileAccessRecord::from(event));
    }

    pub fn take(&self) -> Vec<FileAccessRecord> {
        std::mem::take(
            &mut *self
                .events
                .lock()
                .expect("file access collector mutex poisoned"),
        )
    }

    /// Number of records collected so far (without draining).
    pub fn len(&self) -> usize {
        self.events
            .lock()
            .expect("file access collector mutex poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn converts_file_access_event_to_record() {
        let event = FileAccessEvent {
            path: PathBuf::from("/tmp/example.txt"),
            operation: FileOperation::Write,
            result: FileAccessResult::DeniedByLandlock,
            pid: 4242,
            timestamp: UNIX_EPOCH + Duration::from_secs(1),
        };

        let record = FileAccessRecord::from(event);
        assert_eq!(record.path, "/tmp/example.txt");
        assert_eq!(record.operation, FileAccessOperation::Write);
        assert_eq!(record.result, FileAccessOutcome::DeniedByLandlock);
        assert_eq!(record.pid, 4242);
        assert_eq!(record.timestamp_nanos, 1_000_000_000);
    }

    #[test]
    fn collector_records_and_takes_events() {
        let collector = FileAccessCollector::new();
        collector.record(FileAccessEvent {
            path: PathBuf::from("/etc/passwd"),
            operation: FileOperation::Read,
            result: FileAccessResult::Allowed,
            pid: 1,
            timestamp: UNIX_EPOCH,
        });

        let events = collector.take();
        assert_eq!(events.len(), 1);
        assert!(collector.take().is_empty());
    }
}
