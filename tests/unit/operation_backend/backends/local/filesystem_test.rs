use super::*;
use crate::backends::local::backend::LocalBackendState;
use crate::backends::local::policy::LocalBackendPolicy;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(0);

fn test_root() -> PathBuf {
    let sequence = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "xiaoo-local-fs-policy-{}-{sequence}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(root.as_path());
    std::fs::create_dir_all(root.join("workspace")).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();
    root
}

fn filesystem(root: &Path) -> LocalFileSystem {
    let workspace = root.join("workspace");
    let temp = workspace.join("tmp");
    std::fs::create_dir_all(temp.as_path()).unwrap();
    LocalFileSystem::new(Arc::new(LocalBackendState {
        backend_id: "local-test".to_string(),
        workspace_root: BackendPath::from_raw(workspace.display().to_string()),
        workspace_root_host: workspace.clone(),
        temp_root_host: temp.clone(),
        policy: LocalBackendPolicy::test_macos_seatbelt(
            vec![workspace.clone(), temp.clone()],
            vec![temp],
            false,
        ),
        ..Default::default()
    }))
}

#[test]
fn read_outside_policy_root_is_denied() {
    let root = test_root();
    let file = root.join("outside").join("secret.txt");
    std::fs::write(file.as_path(), b"secret").unwrap();
    let fs = filesystem(root.as_path());

    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(fs.read_bytes(ReadBytesRequest {
            path: BackendPath::from_raw(file.display().to_string()),
        }));

    assert!(matches!(
        result,
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn write_outside_writable_root_is_denied() {
    let root = test_root();
    let file = root.join("workspace").join("src.txt");
    let fs = filesystem(root.as_path());

    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(fs.write_bytes(WriteBytesRequest {
            path: BackendPath::from_raw(file.display().to_string()),
            content: b"change".to_vec(),
            mode: WriteMode::Overwrite,
        }));

    assert!(matches!(
        result,
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
    let _ = std::fs::remove_dir_all(root.as_path());
}
