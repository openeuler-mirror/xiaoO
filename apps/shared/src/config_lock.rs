//! Cross-process advisory lock for config-file read-modify-write cycles,
//! mirroring the secrets store's `StoreLock`.

use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Exclusive advisory lock; released by the OS on process exit.
pub struct ConfigFileLock {
    _file: File,
}

impl ConfigFileLock {
    pub fn acquire(config_path: &Path) -> Result<Self> {
        let lock_path = hidden_sibling(config_path, "lock");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open config lock file {}", lock_path.display()))?;
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is owned by `file` and stays open for as long as the
        // lock is held; `flock` only manipulates the lock state of that fd.
        let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("failed to acquire config lock {}", lock_path.display()));
        }
        Ok(Self { _file: file })
    }
}

/// Hidden sibling `.{file_name}.{suffix}`, so atomic swaps stay on one filesystem.
pub fn hidden_sibling(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config.toml".to_string());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{file_name}.{suffix}"))
}
