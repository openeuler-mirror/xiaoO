//! Request types for execution.
//!
//! This module defines the data structures that describe "what to execute".

use std::path::PathBuf;

/// Describes a command execution request.
///
/// `ExecRequest` is intentionally generic and not bound to shell execution.
/// Shell commands are handled by higher-level helpers, not by this core type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    /// The program to execute (path or name for PATH lookup).
    pub program: String,
    /// Arguments to pass to the program.
    pub args: Vec<String>,
    /// Working directory for the execution. Defaults to current directory if None.
    pub cwd: Option<PathBuf>,
    /// Environment variables as (key, value) pairs.
    pub env: Vec<(String, String)>,
    /// How to handle stdin for the process.
    pub stdin: StdinPolicy,
    /// How to handle stdout for the process.
    pub stdout: OutputPolicy,
    /// How to handle stderr for the process.
    pub stderr: OutputPolicy,
}

/// Policy for handling stdin in the executed process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum StdinPolicy {
    /// Inherit stdin from the parent process.
    Inherit,
    /// Close stdin (equivalent to /dev/null).
    #[default]
    Null,
    /// Provide specific bytes as stdin.
    Bytes(Vec<u8>),
}

/// Policy for handling stdout/stderr in the executed process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OutputPolicy {
    /// Inherit from parent process (direct passthrough).
    #[default]
    Inherit,
    /// Capture to buffer (for filtering/auditing).
    Capture,
}

impl ExecRequest {
    /// Creates a new execution request for the given program.
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            stdin: StdinPolicy::default(),
            stdout: OutputPolicy::default(),
            stderr: OutputPolicy::default(),
        }
    }

    /// Adds an argument to the request.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Adds multiple arguments to the request.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Sets the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Adds multiple environment variables.
    pub fn envs<I, K, V>(mut self, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.env
            .extend(envs.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// Sets the stdin policy.
    pub fn stdin(mut self, stdin: StdinPolicy) -> Self {
        self.stdin = stdin;
        self
    }

    /// Sets the stdout policy.
    pub fn stdout(mut self, stdout: OutputPolicy) -> Self {
        self.stdout = stdout;
        self
    }

    /// Sets the stderr policy.
    pub fn stderr(mut self, stderr: OutputPolicy) -> Self {
        self.stderr = stderr;
        self
    }

    /// Enables output capture for both stdout and stderr.
    pub fn capture_output(mut self) -> Self {
        self.stdout = OutputPolicy::Capture;
        self.stderr = OutputPolicy::Capture;
        self
    }
}
