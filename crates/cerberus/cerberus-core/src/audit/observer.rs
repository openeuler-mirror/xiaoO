//! Observer trait for execution events.
//!
//! Provides hooks for observing and reacting to execution lifecycle events.

use crate::error::AuditError;
use crate::result::ExecResult;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{AuditEvent, CoreExecEvent, EventType, TimeoutEvent};

/// Observer trait for execution events.
///
/// Implementations receive callbacks at various points during execution.
/// Observers are entirely optional - execution works without any observer.
pub trait ExecObserver: Send + Sync {
    /// Called when a request is received.
    fn on_request_received(&self, event: &CoreExecEvent) -> Result<(), AuditError>;

    /// Called when execution starts.
    fn on_execution_started(&self, event: &CoreExecEvent) -> Result<(), AuditError>;

    /// Called when a chunk of output is received.
    fn on_output_chunk(
        &self,
        event: &CoreExecEvent,
        chunk: &[u8],
        is_stderr: bool,
    ) -> Result<(), AuditError>;

    /// Called when execution completes.
    fn on_execution_completed(
        &self,
        event: &CoreExecEvent,
        result: &ExecResult,
    ) -> Result<(), AuditError>;

    /// Called when execution fails.
    fn on_execution_failed(&self, event: &CoreExecEvent, error: &str) -> Result<(), AuditError>;

    /// Called when execution times out and the process is killed.
    fn on_execution_timed_out(
        &self,
        event: &CoreExecEvent,
        timeout: &TimeoutEvent,
    ) -> Result<(), AuditError>;

    /// Clone the observer as a trait object.
    fn clone_box(&self) -> Box<dyn ExecObserver>;
}

impl Clone for Box<dyn ExecObserver> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// A no-op observer that does nothing.
///
/// Use this when you need to satisfy the observer interface but
/// don't want any observation behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOpObserver;

impl NoOpObserver {
    /// Create a new no-op observer.
    pub fn new() -> Self {
        Self
    }
}

impl ExecObserver for NoOpObserver {
    fn on_request_received(&self, _event: &CoreExecEvent) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_execution_started(&self, _event: &CoreExecEvent) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_output_chunk(
        &self,
        _event: &CoreExecEvent,
        _chunk: &[u8],
        _is_stderr: bool,
    ) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_execution_completed(
        &self,
        _event: &CoreExecEvent,
        _result: &ExecResult,
    ) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_execution_failed(&self, _event: &CoreExecEvent, _error: &str) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_execution_timed_out(
        &self,
        _event: &CoreExecEvent,
        _timeout: &TimeoutEvent,
    ) -> Result<(), AuditError> {
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn ExecObserver> {
        Box::new(*self)
    }
}

/// An observer that logs events to a sink.
pub struct LoggingObserver {
    sink: Box<dyn crate::audit::AuditSink>,
}

impl LoggingObserver {
    /// Create a new logging observer with the given sink.
    pub fn new(sink: Box<dyn crate::audit::AuditSink>) -> Self {
        Self { sink }
    }

    fn write_event(&self, event: &CoreExecEvent, event_type: &str) -> Result<(), AuditError> {
        let audit_event = AuditEvent::from_core_event(event, event_type);
        self.sink.write(&audit_event)
    }
}

impl ExecObserver for LoggingObserver {
    fn on_request_received(&self, event: &CoreExecEvent) -> Result<(), AuditError> {
        self.write_event(event, "request_received")
    }

    fn on_execution_started(&self, event: &CoreExecEvent) -> Result<(), AuditError> {
        self.write_event(event, "execution_started")
    }

    fn on_output_chunk(
        &self,
        event: &CoreExecEvent,
        chunk: &[u8],
        is_stderr: bool,
    ) -> Result<(), AuditError> {
        let event_type = if is_stderr {
            "stderr_chunk"
        } else {
            "stdout_chunk"
        };
        let mut audit_event = AuditEvent::from_core_event(event, event_type);

        // Add chunk info to data
        if let serde_json::Value::Object(ref mut map) = audit_event.data {
            map.insert("chunk_size".to_string(), serde_json::json!(chunk.len()));
            map.insert("is_stderr".to_string(), serde_json::json!(is_stderr));
        }

        self.sink.write(&audit_event)
    }

    fn on_execution_completed(
        &self,
        event: &CoreExecEvent,
        result: &ExecResult,
    ) -> Result<(), AuditError> {
        let mut audit_event = AuditEvent::from_core_event(event, "execution_completed");

        // Add result info to data
        if let serde_json::Value::Object(ref mut map) = audit_event.data {
            map.insert("exit_code".to_string(), serde_json::json!(result.exit_code));
            map.insert(
                "duration_ms".to_string(),
                serde_json::json!(result.duration.as_millis()),
            );
            map.insert(
                "stdout_size".to_string(),
                serde_json::json!(result.stdout.len()),
            );
            map.insert(
                "stderr_size".to_string(),
                serde_json::json!(result.stderr.len()),
            );
        }

        self.sink.write(&audit_event)
    }

    fn on_execution_failed(&self, event: &CoreExecEvent, error: &str) -> Result<(), AuditError> {
        let mut audit_event =
            AuditEvent::from_core_event(event, EventType::ExecutionFailed.as_str());

        if let serde_json::Value::Object(ref mut map) = audit_event.data {
            map.insert("error".to_string(), serde_json::json!(error));
        }

        self.sink.write(&audit_event)
    }

    fn on_execution_timed_out(
        &self,
        event: &CoreExecEvent,
        timeout: &TimeoutEvent,
    ) -> Result<(), AuditError> {
        let mut audit_event =
            AuditEvent::from_core_event(event, EventType::ExecutionTimedOut.as_str());

        if let serde_json::Value::Object(ref mut map) = audit_event.data {
            map.insert(
                "duration_ms".to_string(),
                serde_json::json!(timeout.duration.as_millis()),
            );
            map.insert("pid".to_string(), serde_json::json!(timeout.pid));
        }

        self.sink.write(&audit_event)
    }

    fn clone_box(&self) -> Box<dyn ExecObserver> {
        Box::new(Self {
            sink: self.sink.clone(),
        })
    }
}

/// A composite observer that calls multiple observers.
pub struct CompositeObserver {
    observers: Vec<Box<dyn ExecObserver>>,
}

impl CompositeObserver {
    /// Create a new composite observer.
    pub fn new(observers: Vec<Box<dyn ExecObserver>>) -> Self {
        Self { observers }
    }

    /// Add an observer.
    pub fn add(&mut self, observer: Box<dyn ExecObserver>) {
        self.observers.push(observer);
    }
}

impl ExecObserver for CompositeObserver {
    fn on_request_received(&self, event: &CoreExecEvent) -> Result<(), AuditError> {
        for observer in &self.observers {
            observer.on_request_received(event)?;
        }
        Ok(())
    }

    fn on_execution_started(&self, event: &CoreExecEvent) -> Result<(), AuditError> {
        for observer in &self.observers {
            observer.on_execution_started(event)?;
        }
        Ok(())
    }

    fn on_output_chunk(
        &self,
        event: &CoreExecEvent,
        chunk: &[u8],
        is_stderr: bool,
    ) -> Result<(), AuditError> {
        for observer in &self.observers {
            observer.on_output_chunk(event, chunk, is_stderr)?;
        }
        Ok(())
    }

    fn on_execution_completed(
        &self,
        event: &CoreExecEvent,
        result: &ExecResult,
    ) -> Result<(), AuditError> {
        for observer in &self.observers {
            observer.on_execution_completed(event, result)?;
        }
        Ok(())
    }

    fn on_execution_failed(&self, event: &CoreExecEvent, error: &str) -> Result<(), AuditError> {
        for observer in &self.observers {
            observer.on_execution_failed(event, error)?;
        }
        Ok(())
    }

    fn on_execution_timed_out(
        &self,
        event: &CoreExecEvent,
        timeout: &TimeoutEvent,
    ) -> Result<(), AuditError> {
        for observer in &self.observers {
            observer.on_execution_timed_out(event, timeout)?;
        }
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn ExecObserver> {
        Box::new(Self {
            observers: self.observers.iter().map(|o| o.clone_box()).collect(),
        })
    }
}

/// Metrics collected during execution observation.
#[derive(Debug, Clone, Default)]
pub struct ExecutionMetrics {
    /// Time from request to execution start.
    pub queue_time: Option<Duration>,
    /// Time from execution start to completion.
    pub execution_time: Option<Duration>,
    /// Total bytes written to stdout.
    pub stdout_bytes: usize,
    /// Total bytes written to stderr.
    pub stderr_bytes: usize,
    /// Number of output chunks received.
    pub chunk_count: usize,
}

/// An observer that collects execution metrics.
#[derive(Default)]
pub struct MetricsObserver {
    metrics: Arc<Mutex<ExecutionMetrics>>,
    start_time: Option<Instant>,
}

impl MetricsObserver {
    /// Create a new metrics observer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the collected metrics.
    pub fn metrics(&self) -> Result<ExecutionMetrics, AuditError> {
        let metrics = self
            .metrics
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock metrics: {}", e)))?;
        Ok(metrics.clone())
    }

    /// Reset the collected metrics.
    pub fn reset(&self) -> Result<(), AuditError> {
        let mut metrics = self
            .metrics
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock metrics: {}", e)))?;
        *metrics = ExecutionMetrics::default();
        Ok(())
    }
}

impl ExecObserver for MetricsObserver {
    fn on_request_received(&self, _event: &CoreExecEvent) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_execution_started(&self, _event: &CoreExecEvent) -> Result<(), AuditError> {
        // Note: In a real implementation, we'd track timing per-execution
        Ok(())
    }

    fn on_output_chunk(
        &self,
        _event: &CoreExecEvent,
        chunk: &[u8],
        is_stderr: bool,
    ) -> Result<(), AuditError> {
        let mut metrics = self
            .metrics
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock metrics: {}", e)))?;

        metrics.chunk_count += 1;
        if is_stderr {
            metrics.stderr_bytes += chunk.len();
        } else {
            metrics.stdout_bytes += chunk.len();
        }

        Ok(())
    }

    fn on_execution_completed(
        &self,
        _event: &CoreExecEvent,
        result: &ExecResult,
    ) -> Result<(), AuditError> {
        let mut metrics = self
            .metrics
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock metrics: {}", e)))?;

        metrics.execution_time = Some(result.duration);
        Ok(())
    }

    fn on_execution_failed(&self, _event: &CoreExecEvent, _error: &str) -> Result<(), AuditError> {
        Ok(())
    }

    fn on_execution_timed_out(
        &self,
        _event: &CoreExecEvent,
        timeout: &TimeoutEvent,
    ) -> Result<(), AuditError> {
        let mut metrics = self
            .metrics
            .lock()
            .map_err(|e| AuditError::SinkFailed(format!("Failed to lock metrics: {}", e)))?;

        metrics.execution_time = Some(timeout.duration);
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn ExecObserver> {
        Box::new(Self {
            metrics: Arc::clone(&self.metrics),
            start_time: self.start_time,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemorySink;
    use std::time::SystemTime;

    fn make_test_event() -> CoreExecEvent {
        CoreExecEvent {
            request_id: "test-123".to_string(),
            program: "echo".to_string(),
            args: vec!["hello".to_string()],
            timestamp: SystemTime::now(),
            cwd: None,
        }
    }

    #[test]
    fn no_op_observer_does_nothing() {
        let observer = NoOpObserver::new();
        let event = make_test_event();
        let result = ExecResult::success();

        assert!(observer.on_request_received(&event).is_ok());
        assert!(observer.on_execution_started(&event).is_ok());
        assert!(observer.on_output_chunk(&event, b"test", false).is_ok());
        assert!(observer.on_execution_completed(&event, &result).is_ok());
        assert!(observer.on_execution_failed(&event, "error").is_ok());
        assert!(observer
            .on_execution_timed_out(
                &event,
                &TimeoutEvent {
                    duration: Duration::from_secs(1),
                    pid: 42,
                    timestamp: SystemTime::now(),
                },
            )
            .is_ok());
    }

    #[test]
    fn logging_observer_writes_to_sink() {
        let sink = MemorySink::new();
        let observer = LoggingObserver::new(Box::new(sink.clone()));
        let event = make_test_event();

        observer.on_request_received(&event).unwrap();
        observer.on_execution_started(&event).unwrap();

        assert_eq!(sink.len().unwrap(), 2);
    }

    #[test]
    fn logging_observer_with_result() {
        let sink = MemorySink::new();
        let observer = LoggingObserver::new(Box::new(sink.clone()));
        let event = make_test_event();
        let result = ExecResult::success()
            .stdout(b"hello".to_vec())
            .duration(Duration::from_millis(100));

        observer.on_execution_completed(&event, &result).unwrap();

        let events = sink.events().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].data["exit_code"].is_number());
    }

    #[test]
    fn logging_observer_writes_timeout_event() {
        let sink = MemorySink::new();
        let observer = LoggingObserver::new(Box::new(sink.clone()));
        let event = make_test_event();
        let timeout = TimeoutEvent {
            duration: Duration::from_secs(1),
            pid: 123,
            timestamp: SystemTime::now(),
        };

        observer.on_execution_timed_out(&event, &timeout).unwrap();

        let events = sink.events().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, EventType::ExecutionTimedOut.as_str());
        assert_eq!(events[0].data["pid"], 123);
        assert_eq!(events[0].data["duration_ms"], 1000);
    }

    #[test]
    fn composite_observer_calls_all() {
        let sink1 = MemorySink::new();
        let sink2 = MemorySink::new();

        let observer = CompositeObserver::new(vec![
            Box::new(LoggingObserver::new(Box::new(sink1.clone()))),
            Box::new(LoggingObserver::new(Box::new(sink2.clone()))),
        ]);

        let event = make_test_event();
        observer.on_request_received(&event).unwrap();

        assert_eq!(sink1.len().unwrap(), 1);
        assert_eq!(sink2.len().unwrap(), 1);
    }

    #[test]
    fn metrics_observer_collects_metrics() {
        let observer = MetricsObserver::new();
        let event = make_test_event();
        let result = ExecResult::success()
            .stdout(b"hello".to_vec())
            .duration(Duration::from_millis(50));

        observer.on_output_chunk(&event, b"hello", false).unwrap();
        observer.on_output_chunk(&event, b"error", true).unwrap();
        observer.on_execution_completed(&event, &result).unwrap();

        let metrics = observer.metrics().unwrap();
        assert_eq!(metrics.stdout_bytes, 5);
        assert_eq!(metrics.stderr_bytes, 5);
        assert_eq!(metrics.chunk_count, 2);
        assert_eq!(metrics.execution_time, Some(Duration::from_millis(50)));
    }

    #[test]
    fn observer_clone_box() {
        let observer: Box<dyn ExecObserver> = Box::new(NoOpObserver::new());
        let cloned = observer.clone_box();

        let event = make_test_event();
        assert!(observer.on_request_received(&event).is_ok());
        assert!(cloned.on_request_received(&event).is_ok());
    }
}
