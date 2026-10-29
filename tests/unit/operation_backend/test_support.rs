//! Test-only helpers shared across operation_backend test modules.

use std::sync::Mutex;

pub(crate) fn test_workspace_root(prefix: &str, name: &str) -> std::path::PathBuf {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let root = std::env::temp_dir().join(format!("{prefix}{name}-{}-{millis}", std::process::id()));
    let _ = std::fs::remove_dir_all(root.as_path());
    std::fs::create_dir_all(root.join("workspace")).unwrap();
    root
}

pub(crate) fn process_group_test_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Keep writable fixture descriptors out of this multithreaded test process.
#[cfg(unix)]
pub(crate) fn write_script(path: &std::path::Path, content: &str) {
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", "printf '%s' \"$1\" > \"$2\"", "write-script"])
        .arg(content)
        .arg(path)
        .status()
        .expect("run script writer");
    assert!(status.success(), "write script fixture");
}
