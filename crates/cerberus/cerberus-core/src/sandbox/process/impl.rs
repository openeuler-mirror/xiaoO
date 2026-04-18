//! Process execution with namespace isolation.
//!
//! This module provides utilities for spawning processes with
//! namespace-based isolation. It handles the low-level details
//! of fork, namespace setup, and exec.

use crate::error::SandboxSetupError;
use crate::policy::{NamespaceConfig, ResourceLimits};
use std::path::PathBuf;

/// Options for spawning a sandboxed process.
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// Program to execute.
    pub program: PathBuf,
    /// Arguments to pass to the program.
    pub args: Vec<String>,
    /// Working directory for the process.
    pub cwd: Option<PathBuf>,
    /// Environment variables (KEY=VALUE format).
    pub env: Vec<(String, String)>,
    /// Namespace configuration.
    pub namespaces: NamespaceConfig,
    /// Resource limits.
    pub resources: ResourceLimits,
}

impl SpawnOptions {
    /// Create new spawn options with the given program.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            namespaces: NamespaceConfig::default(),
            resources: ResourceLimits::default(),
        }
    }

    /// Add an argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add an environment variable.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Set namespace configuration.
    pub fn namespaces(mut self, namespaces: NamespaceConfig) -> Self {
        self.namespaces = namespaces;
        self
    }

    /// Set resource limits.
    pub fn resources(mut self, resources: ResourceLimits) -> Self {
        self.resources = resources;
        self
    }
}

/// Sandbox process handle.
///
/// This struct represents a process that has been spawned with
/// sandbox isolation. It provides methods for waiting for
/// completion and retrieving results.
#[derive(Debug)]
pub struct SandboxProcess {
    /// Process ID.
    pub pid: u32,
    /// Whether the process is still running.
    pub running: bool,
}

impl SandboxProcess {
    /// Create a new sandbox process handle.
    pub fn new(pid: u32) -> Self {
        Self { pid, running: true }
    }

    /// Wait for the process to complete.
    ///
    /// On Linux, this uses waitpid. On other platforms, this returns
    /// an error as sandbox processes are not supported.
    #[cfg(target_os = "linux")]
    pub fn wait(&mut self) -> Result<i32, SandboxSetupError> {
        let mut status: i32 = 0;
        let result = unsafe { libc::waitpid(self.pid as i32, &mut status, 0) };

        if result < 0 {
            return Err(SandboxSetupError::NamespaceSetupFailed(format!(
                "waitpid failed: {}",
                std::io::Error::last_os_error()
            )));
        }

        self.running = false;

        // Extract exit code
        if libc::WIFEXITED(status) {
            Ok(libc::WEXITSTATUS(status))
        } else if libc::WIFSIGNALED(status) {
            // Process was killed by signal, return negative signal number
            Ok(-libc::WTERMSIG(status))
        } else {
            Ok(-1)
        }
    }

    /// Wait for the process to complete (non-Linux stub).
    #[cfg(not(target_os = "linux"))]
    pub fn wait(&mut self) -> Result<i32, SandboxSetupError> {
        Err(SandboxSetupError::UnsupportedPlatform)
    }

    /// Kill the process.
    #[cfg(target_os = "linux")]
    pub fn kill(&self) -> Result<(), SandboxSetupError> {
        let result = unsafe { libc::kill(self.pid as i32, libc::SIGKILL) };
        if result < 0 {
            return Err(SandboxSetupError::NamespaceSetupFailed(format!(
                "kill failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Kill the process (non-Linux stub).
    #[cfg(not(target_os = "linux"))]
    pub fn kill(&self) -> Result<(), SandboxSetupError> {
        Err(SandboxSetupError::UnsupportedPlatform)
    }
}

/// Apply resource limits to the current process.
#[cfg(target_os = "linux")]
pub fn apply_resource_limits(resources: &ResourceLimits) -> Result<(), SandboxSetupError> {
    // Set memory limit (only for positive values, skip 0/None)
    if let Some(max_memory) = resources.max_memory_bytes {
        if max_memory > 0 {
            let rlimit = libc::rlimit {
                rlim_cur: max_memory as libc::rlim_t,
                rlim_max: max_memory as libc::rlim_t,
            };
            let result = unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlimit) };
            if result != 0 {
                return Err(SandboxSetupError::NamespaceSetupFailed(format!(
                    "setrlimit(RLIMIT_AS) failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }
    }

    // Set process count limit (only for positive values, skip 0/None)
    if let Some(max_processes) = resources.max_processes {
        if max_processes > 0 {
            let rlimit = libc::rlimit {
                rlim_cur: max_processes as libc::rlim_t,
                rlim_max: max_processes as libc::rlim_t,
            };
            let result = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlimit) };
            if result != 0 {
                return Err(SandboxSetupError::NamespaceSetupFailed(format!(
                    "setrlimit(RLIMIT_NPROC) failed: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }
    }

    Ok(())
}

/// Apply resource limits (non-Linux stub).
#[cfg(not(target_os = "linux"))]
pub fn apply_resource_limits(_resources: &ResourceLimits) -> Result<(), SandboxSetupError> {
    Err(SandboxSetupError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_options_builder() {
        let opts = SpawnOptions::new("/bin/ls")
            .arg("-la")
            .cwd("/tmp")
            .env("FOO", "bar");

        assert_eq!(opts.program, PathBuf::from("/bin/ls"));
        assert_eq!(opts.args, vec!["-la"]);
        assert_eq!(opts.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(opts.env, vec![("FOO".to_string(), "bar".to_string())]);
    }

    #[test]
    fn test_spawn_options_multiple_args() {
        let opts = SpawnOptions::new("/bin/echo").args(["hello", "world"]);

        assert_eq!(opts.args, vec!["hello", "world"]);
    }

    #[test]
    fn test_sandbox_process_new() {
        let process = SandboxProcess::new(1234);
        assert_eq!(process.pid, 1234);
        assert!(process.running);
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_unsupported_platform() {
        let mut process = SandboxProcess::new(1234);
        assert!(process.wait().is_err());
        assert!(process.kill().is_err());
        assert!(apply_resource_limits(&ResourceLimits::default()).is_err());
    }
}
