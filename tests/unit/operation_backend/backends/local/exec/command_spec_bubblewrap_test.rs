use super::*;
use crate::backends::local::factory::local_backend_with_isolation;
use crate::test_support::test_workspace_root;
use agent_contracts::backend::capability::exec::ExecResult;
use agent_contracts::backend::{
    BackendPath, SandboxPermissionCapability, SandboxPermissionGrantRequest,
    SandboxPermissionScope, SandboxPolicyDenial,
};
use serde_json::json;
use std::path::{Path, PathBuf};

#[test]
fn bubblewrap_exec_enforces_filesystem_policy() {
    if !has_bwrap() {
        return;
    }
    // Serialize against the process-group unit tests: `exec` registers real
    // child pgids in the shared global registry, and `kill_all_process_groups`
    // would otherwise clear it (and signal our children) mid-test.
    let _guard = crate::test_support::process_group_test_lock()
        .lock()
        .unwrap();
    let root = test_workspace_root("xiaoo-bubblewrap-", "fs");
    let workspace = root.join("workspace");
    let writable = workspace.join("tmp");
    let outside = root.join("outside");
    std::fs::create_dir_all(writable.as_path()).unwrap();
    std::fs::create_dir_all(outside.as_path()).unwrap();
    std::fs::write(workspace.join("readable.txt"), b"visible").unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

    let backend = bubblewrap_backend(workspace.clone(), writable.clone(), false);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let read = runtime.block_on(exec_bash(backend.as_ref(), &workspace, "cat readable.txt"));
    assert_eq!(read.exit_code, Some(0));
    assert_eq!(String::from_utf8_lossy(read.stdout.as_slice()), "visible");

    let write = runtime.block_on(exec_bash(
        backend.as_ref(),
        &workspace,
        "printf ok > tmp/out.txt",
    ));
    assert_eq!(write.exit_code, Some(0));
    assert_eq!(
        std::fs::read_to_string(writable.join("out.txt")).unwrap(),
        "ok"
    );

    let denied_write = runtime.block_on(exec_bash(
        backend.as_ref(),
        &workspace,
        "printf no > blocked.txt",
    ));
    assert_ne!(denied_write.exit_code, Some(0));
    assert!(!workspace.join("blocked.txt").exists());

    let outside_path = outside.join("secret.txt");
    let denied_read = runtime.block_on(exec_bash(
        backend.as_ref(),
        &workspace,
        format!("cat {}", shell_quote(outside_path.as_path())).as_str(),
    ));
    assert_ne!(denied_read.exit_code, Some(0));
    assert!(!String::from_utf8_lossy(denied_read.stdout.as_slice()).contains("secret"));

    backend
        .permission_control()
        .unwrap()
        .grant(SandboxPermissionGrantRequest {
            denial: SandboxPolicyDenial {
                backend_id: backend.backend_id().to_string(),
                isolation: "linux_bubblewrap".to_string(),
                operation: "test".to_string(),
                capability: SandboxPermissionCapability::Read,
                path: outside_path.display().to_string(),
            },
            scope: SandboxPermissionScope::Session,
        })
        .unwrap();

    let granted_read = runtime.block_on(exec_bash(
        backend.as_ref(),
        &workspace,
        format!("cat {}", shell_quote(outside_path.as_path())).as_str(),
    ));
    assert_eq!(granted_read.exit_code, Some(0));
    assert_eq!(
        String::from_utf8_lossy(granted_read.stdout.as_slice()),
        "secret"
    );

    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn bubblewrap_exec_can_unshare_network() {
    if !has_bwrap() {
        return;
    }
    let _guard = crate::test_support::process_group_test_lock()
        .lock()
        .unwrap();
    let root = test_workspace_root("xiaoo-bubblewrap-", "net");
    let workspace = root.join("workspace");
    let writable = workspace.join("tmp");
    std::fs::create_dir_all(writable.as_path()).unwrap();

    let backend = bubblewrap_backend(workspace.clone(), writable, false);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(exec_bash(backend.as_ref(), &workspace, "cat /proc/net/dev"));

    assert_eq!(result.exit_code, Some(0));
    let interfaces = String::from_utf8_lossy(result.stdout.as_slice())
        .lines()
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, _)| name.trim().to_string())
        })
        .collect::<Vec<_>>();
    assert!(
        interfaces.iter().all(|name| name == "lo"),
        "unexpected interfaces in unshared network namespace: {interfaces:?}"
    );

    let _ = std::fs::remove_dir_all(root.as_path());
}

fn bubblewrap_backend(
    workspace: PathBuf,
    writable: PathBuf,
    allow_network: bool,
) -> std::sync::Arc<dyn agent_contracts::backend::OperationBackend> {
    local_backend_with_isolation(
        workspace.clone(),
        None,
        Some(writable.clone()),
        None,
        Some(json!({
            "kind": "linux_bubblewrap",
            "allow_network": allow_network,
            "readable_roots": [workspace.to_string_lossy().to_string()],
            "writable_roots": [writable.to_string_lossy().to_string()]
        })),
    )
    .unwrap()
}

async fn exec_bash(
    backend: &dyn agent_contracts::backend::OperationBackend,
    cwd: &Path,
    command: &str,
) -> ExecResult {
    backend
        .exec()
        .exec(ExecRequest {
            command: command.to_string(),
            args: vec![],
            shell: Some("bash".to_string()),
            cwd: Some(BackendPath::from_raw(cwd.to_string_lossy().into_owned())),
            timeout_ms: Some(5_000),
            ..Default::default()
        })
        .await
        .unwrap()
}

fn has_bwrap() -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("bwrap").is_file()))
        .unwrap_or(false)
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}
