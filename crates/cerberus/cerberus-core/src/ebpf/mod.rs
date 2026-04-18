//! eBPF audit backend module for Cerberus.
//!
//! This module provides an eBPF-based audit backend that monitors system calls
//! and security events in the kernel. It is only available on Linux with the
//! `ebpf` feature enabled.

mod backend;
mod loader;

pub use backend::{EbpfAuditBackend, EbpfAuditBackendBuilder};
pub use loader::{EbpfLoadError, EbpfLoader};
