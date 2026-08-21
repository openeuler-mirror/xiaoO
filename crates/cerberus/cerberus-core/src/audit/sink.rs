//! Audit sink trait and implementations.
//!
//! Provides the abstraction for recording audit events to various
//! destinations (files, network, memory, etc.).

use crate::error::AuditError;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::AuditEvent;

/// Trait for audit event sinks.
///
/// Sinks receive audit events and write them to some destination.
/// Implementations should handle errors gracefully and not panic.
pub trait AuditSink: Send + Sync {
    /// Write an audit event to the sink.
    fn write(&self, event: &AuditEvent) -> Result<(), AuditError>;

    /// Flush any buffered data.
    fn flush(&self) -> Result<(), AuditError>;

    /// Clone the sink as a trait object.
    fn clone_box(&self) -> Box<dyn AuditSink>;
}

impl Clone for Box<dyn AuditSink> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// A no-op sink that discards all events.
///
/// Use this when audit logging is disabled but you need to satisfy
/// the sink interface.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpSink;

impl NoOpSink {
    /// Create a new no-op sink.
    pub fn new() -> Self {
        Self
    }
}

impl AuditSink for NoOpSink {
    fn write(&self, _event: &AuditEvent) -> Result<(), AuditError> {
        Ok(())
    }

    fn flush(&self) -> Result<(), AuditError> {
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn AuditSink> {
        Box::new(*self)
    }
}

/// A sink that writes events to a file.
///
/// Each event is written as a single line of JSON.
#[derive(Debug)]
pub struct FileSink {
    file: Arc<Mutex<File>>,
    path: PathBuf,
}

impl FileSink {
    /// Create a new file sink.
    ///
    /// Opens the file in append mode, creating it if it doesn't exist.
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, AuditError> {
        let path = path.into();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| {
                AuditError::SinkFailed(format!("Failed to open {}: {}", path.display(), e))
            })?;

        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            path,
        })
    }

    /// Get the file path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl AuditSink for FileSink {
    fn write(&self, event: &AuditEvent) -> Result<(), AuditError> {
        let mut file = self
            .file
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock file: {}", e)))?;

        let json = serde_json::to_string(event)
            .map_err(|e| AuditError::EventEncodingFailed(e.to_string()))?;

        writeln!(file, "{}", json)
            .map_err(|e| AuditError::SinkFailed(format!("Write failed: {}", e)))?;

        Ok(())
    }

    fn flush(&self) -> Result<(), AuditError> {
        let mut file = self
            .file
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock file: {}", e)))?;

        file.flush()
            .map_err(|e| AuditError::SinkFailed(format!("Flush failed: {}", e)))?;

        Ok(())
    }

    fn clone_box(&self) -> Box<dyn AuditSink> {
        Box::new(Self {
            file: Arc::clone(&self.file),
            path: self.path.clone(),
        })
    }
}

/// A sink that stores events in memory.
///
/// Useful for testing and debugging.
#[derive(Debug, Clone, Default)]
pub struct MemorySink {
    events: Arc<Mutex<Vec<AuditEvent>>>,
}

impl MemorySink {
    /// Create a new memory sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get all recorded events.
    pub fn events(&self) -> Result<Vec<AuditEvent>, AuditError> {
        let events = self
            .events
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock: {}", e)))?;
        Ok(events.clone())
    }

    /// Get the number of recorded events.
    pub fn len(&self) -> Result<usize, AuditError> {
        let events = self
            .events
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock: {}", e)))?;
        Ok(events.len())
    }

    /// Check if there are no events.
    pub fn is_empty(&self) -> Result<bool, AuditError> {
        Ok(self.len()? == 0)
    }

    /// Clear all recorded events.
    pub fn clear(&self) -> Result<(), AuditError> {
        let mut events = self
            .events
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock: {}", e)))?;
        events.clear();
        Ok(())
    }
}

impl AuditSink for MemorySink {
    fn write(&self, event: &AuditEvent) -> Result<(), AuditError> {
        let mut events = self
            .events
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock: {}", e)))?;
        events.push(event.clone());
        Ok(())
    }

    fn flush(&self) -> Result<(), AuditError> {
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn AuditSink> {
        Box::new(Self {
            events: Arc::clone(&self.events),
        })
    }
}

/// A sink that writes to multiple sinks.
pub struct MultiSink {
    sinks: Vec<Box<dyn AuditSink>>,
}

impl MultiSink {
    /// Create a new multi-sink.
    pub fn new(sinks: Vec<Box<dyn AuditSink>>) -> Self {
        Self { sinks }
    }

    /// Add a sink.
    pub fn add(&mut self, sink: Box<dyn AuditSink>) {
        self.sinks.push(sink);
    }
}

impl AuditSink for MultiSink {
    fn write(&self, event: &AuditEvent) -> Result<(), AuditError> {
        for sink in &self.sinks {
            sink.write(event)?;
        }
        Ok(())
    }

    fn flush(&self) -> Result<(), AuditError> {
        for sink in &self.sinks {
            sink.flush()?;
        }
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn AuditSink> {
        Box::new(Self {
            sinks: self.sinks.iter().map(|s| s.clone_box()).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn make_test_event() -> AuditEvent {
        AuditEvent {
            id: "test-id".to_string(),
            timestamp: SystemTime::now(),
            event_type: "test".to_string(),
            data: serde_json::Value::Null,
        }
    }

    #[test]
    fn no_op_sink_discards_events() {
        let sink = NoOpSink::new();
        let event = make_test_event();

        assert!(sink.write(&event).is_ok());
        assert!(sink.flush().is_ok());
    }

    #[test]
    fn memory_sink_stores_events() {
        let sink = MemorySink::new();
        let event = make_test_event();

        assert!(sink.write(&event).is_ok());
        assert_eq!(sink.len().unwrap(), 1);

        let events = sink.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "test-id");
    }

    #[test]
    fn memory_sink_clear() {
        let sink = MemorySink::new();
        let event = make_test_event();

        sink.write(&event).unwrap();
        assert_eq!(sink.len().unwrap(), 1);

        sink.clear().unwrap();
        assert!(sink.is_empty().unwrap());
    }

    #[test]
    fn multi_sink_writes_to_all() {
        let sink1 = MemorySink::new();
        let sink2 = MemorySink::new();

        let multi = MultiSink::new(vec![Box::new(sink1.clone()), Box::new(sink2.clone())]);

        let event = make_test_event();
        multi.write(&event).unwrap();

        assert_eq!(sink1.len().unwrap(), 1);
        assert_eq!(sink2.len().unwrap(), 1);
    }

    #[test]
    fn file_sink_writes_to_file() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("cerberus_test_audit.log");

        // Clean up any existing file
        let _ = std::fs::remove_file(&path);

        let sink = FileSink::new(&path).expect("should create file sink");
        let event = make_test_event();

        sink.write(&event).expect("should write event");
        sink.flush().expect("should flush");

        // Read file and verify
        let contents = std::fs::read_to_string(&path).expect("should read file");
        assert!(contents.contains("test-id"));

        // Clean up
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sink_clone_box() {
        let sink: Box<dyn AuditSink> = Box::new(MemorySink::new());
        let cloned = sink.clone_box();

        // Both should work independently
        let event = make_test_event();
        sink.write(&event).unwrap();
        cloned.write(&event).unwrap();
    }
}
