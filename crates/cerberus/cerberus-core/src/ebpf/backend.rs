//! eBPF audit backend implementation.

use crate::audit::EbpfAuditEvent;
use crate::ebpf::loader::EbpfLoader;
use std::sync::Arc;
use tokio::sync::mpsc;

/// eBPF-based audit backend for receiving kernel-level security events.
pub struct EbpfAuditBackend {
    event_rx: mpsc::Receiver<EbpfAuditEvent>,
    callback: Option<Arc<dyn Fn(&EbpfAuditEvent) + Send + Sync>>,
}

impl EbpfAuditBackend {
    /// Create a new eBPF audit backend.
    ///
    /// This loads and attaches the eBPF programs and opens perf buffers
    /// for receiving events from the kernel.
    ///
    /// # Errors
    ///
    /// Returns `EbpfLoadError` if the eBPF programs cannot be loaded or attached.
    pub fn new() -> Result<Self, crate::ebpf::EbpfLoadError> {
        let mut loader = EbpfLoader::load()?;
        loader.attach()?;
        let event_rx = loader.open_perf_buffers(1024)?;

        Ok(Self {
            event_rx,
            callback: None,
        })
    }

    /// Create an eBPF audit backend with a pre-configured loader.
    pub fn with_loader(mut loader: EbpfLoader) -> Result<Self, crate::ebpf::EbpfLoadError> {
        loader.attach()?;
        let event_rx = loader.open_perf_buffers(1024)?;

        Ok(Self {
            event_rx,
            callback: None,
        })
    }

    /// Set a callback to be invoked for each audit event.
    pub fn set_callback<F>(&mut self, callback: F)
    where
        F: Fn(&EbpfAuditEvent) + Send + Sync + 'static,
    {
        self.callback = Some(Arc::new(callback));
    }

    /// Receive the next audit event.
    ///
    /// Returns `None` if the event channel is closed.
    pub async fn next_event(&mut self) -> Option<EbpfAuditEvent> {
        self.event_rx.recv().await
    }

    /// Run the event loop, calling the callback for each event.
    pub async fn run_event_loop(&mut self) {
        while let Some(event) = self.event_rx.recv().await {
            if let Some(ref callback) = self.callback {
                callback(&event);
            }
        }
    }
}

/// Builder for configuring an eBPF audit backend.
pub struct EbpfAuditBackendBuilder {
    callback: Option<Arc<dyn Fn(&EbpfAuditEvent) + Send + Sync>>,
    buffer_size: usize,
}

impl EbpfAuditBackendBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            callback: None,
            buffer_size: 1024,
        }
    }

    /// Set the callback to be invoked for each audit event.
    pub fn callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(&EbpfAuditEvent) + Send + Sync + 'static,
    {
        self.callback = Some(Arc::new(callback));
        self
    }

    /// Set the perf buffer size.
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Build the eBPF audit backend.
    pub fn build(self) -> Result<EbpfAuditBackend, crate::ebpf::EbpfLoadError> {
        let mut backend = EbpfAuditBackend::new()?;
        backend.callback = self.callback;
        Ok(backend)
    }
}

impl Default for EbpfAuditBackendBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_new() {
        let builder = EbpfAuditBackendBuilder::new();
        assert!(builder.callback.is_none());
        assert_eq!(builder.buffer_size, 1024);
    }

    #[test]
    fn test_builder_with_buffer_size() {
        let builder = EbpfAuditBackendBuilder::new().buffer_size(2048);
        assert_eq!(builder.buffer_size, 2048);
    }
}
