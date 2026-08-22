//! Cerberus CLI library.
//!
//! This module exports the CLI internals for integration testing.

pub mod app;
pub mod profiles;

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::{LazyLock, Mutex, MutexGuard};

    static CWD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    pub(crate) fn lock_cwd() -> MutexGuard<'static, ()> {
        CWD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn current_dir_or_manifest_dir() -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
    }
}
