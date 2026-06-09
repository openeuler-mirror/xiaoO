//! Result types for execution.
//!
//! This module defines the data structures that describe execution outcomes.

use crate::audit::{FileAccessRecord, NetworkAccessRecord};
use std::time::Duration;

/// Describes the result of a command execution.
///
/// Output fields are byte-oriented (`Vec<u8>`) rather than UTF-8 strings,
/// avoiding premature encoding assumptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    /// The exit code of the process.
    pub exit_code: i32,
    /// Captured stdout bytes.
    pub stdout: Vec<u8>,
    /// Captured stderr bytes.
    pub stderr: Vec<u8>,
    /// Duration of the execution.
    pub duration: Duration,
    /// Additional metadata about the execution.
    pub metadata: ExecMetadata,
    /// File access events collected during execution when policy enables audit.
    pub file_accesses: Vec<FileAccessRecord>,
    /// Network access events collected during execution when a network policy
    /// is active.
    pub network_accesses: Vec<NetworkAccessRecord>,
}

/// Metadata about execution behavior and control actions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecMetadata {
    /// Whether the execution timed out.
    pub timed_out: bool,
    /// Whether the process was killed.
    pub killed: bool,
    /// Signal number if killed by signal, if available.
    pub signal: Option<i32>,
    /// Number of output redactions applied.
    pub redactions_applied: usize,
}

impl ExecResult {
    /// Creates a new execution result with the given exit code.
    pub fn new(exit_code: i32) -> Self {
        Self {
            exit_code,
            stdout: Vec::new(),
            stderr: Vec::new(),
            duration: Duration::ZERO,
            metadata: ExecMetadata::default(),
            file_accesses: Vec::new(),
            network_accesses: Vec::new(),
        }
    }

    /// Creates a successful execution result (exit code 0).
    pub fn success() -> Self {
        Self::new(0)
    }

    /// Returns true if the execution succeeded (exit code 0).
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }

    /// Returns stdout as a UTF-8 string, lossily converting invalid bytes.
    pub fn stdout_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Returns stderr as a UTF-8 string, lossily converting invalid bytes.
    pub fn stderr_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// Sets stdout bytes.
    pub fn stdout(mut self, stdout: Vec<u8>) -> Self {
        self.stdout = stdout;
        self
    }

    /// Sets stderr bytes.
    pub fn stderr(mut self, stderr: Vec<u8>) -> Self {
        self.stderr = stderr;
        self
    }

    /// Sets duration.
    pub fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Sets metadata.
    pub fn metadata(mut self, metadata: ExecMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    /// Sets collected file access audit records.
    pub fn file_accesses(mut self, file_accesses: Vec<FileAccessRecord>) -> Self {
        self.file_accesses = file_accesses;
        self
    }

    /// Sets collected network access audit records.
    pub fn network_accesses(mut self, network_accesses: Vec<NetworkAccessRecord>) -> Self {
        self.network_accesses = network_accesses;
        self
    }
}

impl ExecMetadata {
    /// Creates new metadata with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Marks the execution as timed out.
    pub fn timed_out(mut self) -> Self {
        self.timed_out = true;
        self
    }

    /// Marks the process as killed.
    pub fn killed(mut self) -> Self {
        self.killed = true;
        self
    }

    /// Sets the signal number.
    pub fn signal(mut self, signal: i32) -> Self {
        self.signal = Some(signal);
        self
    }

    /// Sets the number of redactions applied.
    pub fn redactions(mut self, count: usize) -> Self {
        self.redactions_applied = count;
        self
    }
}
