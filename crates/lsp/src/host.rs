use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::process::{Child, ChildStdin, ChildStdout};

use agent_contracts::backend::OperationBackend;
use agent_types::lsp::LspError;

/// The stdio handles and process handle for a spawned LSP server process.
pub struct SpawnedProcess {
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub child: Child,
}

/// Abstracts the host-level operations needed by the LSP subsystem.
///
/// Overlapping functionality (file I/O, command execution, path existence) is
/// delegated to [`OperationBackend`] via [`LspEnv::backend`]. Only the
/// LSP-specific operations that have no backend equivalent remain here:
/// binary resolution via PATH, the LSP-specific install directory, and
/// spawning a long-running piped server process.
#[async_trait]
pub trait LspEnv: Send + Sync {
    /// Search PATH + `global_bin_dir()` for an executable named `cmd`.
    fn which(&self, cmd: &str) -> Option<PathBuf>;

    /// Directory where auto-installed LSP servers are placed.
    fn global_bin_dir(&self) -> PathBuf;

    /// The underlying operation backend — used for file I/O, command
    /// execution, and path existence checks.
    fn backend(&self) -> &dyn OperationBackend;

    /// Spawn a long-running LSP server process with piped stdin/stdout.
    async fn spawn_process(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: &Path,
    ) -> Result<SpawnedProcess, LspError>;
}

// ── Local implementation ──────────────────────────────────────────────────────

/// [`LspEnv`] implementation backed by a local [`OperationBackend`].
///
/// Uses the backend for all file and exec operations, and host-OS APIs
/// (`std::env`, `tokio::process::Command`) for the LSP-specific parts that
/// have no backend equivalent.
pub struct LocalLspEnv {
    backend: Arc<dyn OperationBackend>,
}

impl LocalLspEnv {
    pub fn new(backend: Arc<dyn OperationBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl LspEnv for LocalLspEnv {
    fn which(&self, cmd: &str) -> Option<PathBuf> {
        let path_var = std::env::var("PATH").unwrap_or_default();
        let extra = self.global_bin_dir();
        std::env::split_paths(&path_var)
            .chain(std::iter::once(extra))
            .flat_map(|dir| {
                let candidate = dir.join(cmd);
                #[cfg(windows)]
                let candidates = vec![candidate, dir.join(format!("{}.exe", cmd))];
                #[cfg(not(windows))]
                let candidates = vec![candidate];
                candidates
            })
            .find(|p| p.is_file())
    }

    fn global_bin_dir(&self) -> PathBuf {
        if let Ok(dir) = std::env::var("XIAOO_BIN") {
            return PathBuf::from(dir);
        }
        self.backend
            .paths()
            .home_dir()
            .map(|p| PathBuf::from(p.0.as_str()))
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".local")
            .join("share")
            .join("xiaoo")
            .join("bin")
    }

    fn backend(&self) -> &dyn OperationBackend {
        self.backend.as_ref()
    }

    async fn spawn_process(
        &self,
        cmd: &str,
        args: &[&str],
        cwd: &Path,
    ) -> Result<SpawnedProcess, LspError> {
        let mut child = tokio::process::Command::new(cmd)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        Ok(SpawnedProcess {
            stdin,
            stdout,
            child,
        })
    }
}
