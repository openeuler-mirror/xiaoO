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

/// Install the shared fake `dyn-sandbox` on PATH (idempotent per test
/// process). The real binary is not installed in this environment; the
/// backend's `linux_dynsandbox_available()` PATH probe must pass for
/// `linux_dynsandbox` isolation to build. The script plays the sandbox
/// side of the AUTH protocol: it announces a blocked path on stderr,
/// reads the decision from stdin (the AUTH control channel), and echoes
/// it back so exec tests can observe what was written. Callers that only
/// need the availability probe never exec it, so the protocol behavior
/// is inert for them.
///
/// Single source of truth, deliberately: PATH is process-global and
/// resolution honors the FIRST directory that contains the binary, so
/// per-module installers that each prepend their own directory race for
/// resolution order — a stub from another module must never be able to
/// shadow the AUTH script (or vice versa). One directory, one script,
/// one prepend, guarded by a OnceLock.
#[cfg(target_os = "linux")]
pub(crate) fn install_fake_dyn_sandbox() {
    use std::sync::OnceLock;
    static FAKE_DYN_SANDBOX: OnceLock<()> = OnceLock::new();
    FAKE_DYN_SANDBOX.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "xiaoo-fake-dyn-sandbox-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
        std::fs::create_dir_all(root.as_path()).unwrap();
        let bin = root.join("dyn-sandbox");
        write_script(
            bin.as_path(),
            "#!/bin/sh\necho \"AUTH_REQ:shadow:/etc/shadow\" >&2\nIFS= read -r response\necho \"verdict:$response\"\nexit 0\n",
        );
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.as_path(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let mut paths = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .collect::<Vec<_>>();
        paths.insert(0, root);
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    });
}
