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
            extra: None,
        }));

    assert!(matches!(
        result,
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[cfg(unix)]
async fn product_write_then_exec(mode: WriteMode) {
    use agent_contracts::backend::capability::exec::ExecRequest;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let backend = crate::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let path = temp.path().join("product-executable.sh");
    if !matches!(mode, WriteMode::Create) {
        let seed = temp.path().join("seed");
        backend
            .files()
            .write_bytes(WriteBytesRequest {
                path: BackendPath::from_raw(seed.display().to_string()),
                content: b"#!/bin/sh\nexit 0\n".to_vec(),
                mode: WriteMode::Create,
                extra: None,
            })
            .await
            .unwrap();
        std::fs::rename(seed, &path).unwrap();
    }
    let stopped = Arc::new(AtomicBool::new(false));
    let spawns = std::thread::spawn({
        let backend = Arc::clone(&backend);
        let stopped = Arc::clone(&stopped);
        move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    for _ in 0..512 {
                        if stopped.load(Ordering::Acquire) {
                            break;
                        }
                        backend
                            .exec()
                            .exec(ExecRequest {
                                command: "/bin/true".into(),
                                ..Default::default()
                            })
                            .await
                            .unwrap();
                    }
                })
        }
    });
    tokio::task::yield_now().await;
    let outcome = backend
        .files()
        .write_bytes(WriteBytesRequest {
            path: BackendPath::from_raw(path.display().to_string()),
            content: b"#!/bin/sh\nexit 0\n".to_vec(),
            mode,
            extra: None,
        })
        .await
        .unwrap();
    println!(
        "WRITE_RETURNED mode={mode:?} path={} created={}",
        path.display(),
        outcome.created
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let result = backend
        .exec()
        .exec(ExecRequest {
            command: path.display().to_string(),
            ..Default::default()
        })
        .await;
    stopped.store(true, Ordering::Release);
    spawns.join().unwrap();
    assert_eq!(
        result
            .expect("execute successfully written product file")
            .exit_code,
        Some(0)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn product_create_then_exec_during_concurrent_spawns() {
    product_write_then_exec(WriteMode::Create).await;
}
#[cfg(unix)]
#[tokio::test]
async fn product_overwrite_then_exec_during_concurrent_spawns() {
    product_write_then_exec(WriteMode::Overwrite).await;
}
#[cfg(unix)]
#[tokio::test]
async fn product_atomic_overwrite_then_exec_during_concurrent_spawns() {
    product_write_then_exec(WriteMode::AtomicOverwrite).await;
}

#[cfg(unix)]
#[tokio::test]
async fn product_temp_file_then_write_and_exec_during_concurrent_spawns() {
    use agent_contracts::backend::capability::exec::ExecRequest;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let backend = crate::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let spawns = std::thread::spawn({
        let backend = Arc::clone(&backend);
        let stopped = Arc::clone(&stopped);
        move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    for _ in 0..512 {
                        if stopped.load(Ordering::Acquire) {
                            break;
                        }
                        backend
                            .exec()
                            .exec(ExecRequest {
                                command: "/bin/true".into(),
                                ..Default::default()
                            })
                            .await
                            .unwrap();
                    }
                })
        }
    });
    let path = backend
        .files()
        .temp_path(TempPathRequest {
            kind: TempPathKind::File,
            preferred_parent: Some(BackendPath::from_raw(temp.path().display().to_string())),
            prefix: Some("product-executable".into()),
            suffix: Some(".sh".into()),
        })
        .await
        .unwrap();
    backend
        .files()
        .write_bytes(WriteBytesRequest {
            path: path.clone(),
            content: b"#!/bin/sh\nexit 0\n".to_vec(),
            mode: WriteMode::Overwrite,
            extra: None,
        })
        .await
        .unwrap();
    println!("WRITE_RETURNED temp_path={}", path.native());
    std::fs::set_permissions(path.native(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let result = backend
        .exec()
        .exec(ExecRequest {
            command: path.native().into(),
            ..Default::default()
        })
        .await;
    stopped.store(true, Ordering::Release);
    spawns.join().unwrap();
    assert_eq!(result.unwrap().exit_code, Some(0));
}

#[cfg(unix)]
#[tokio::test]
async fn external_writer_still_causes_text_file_busy() {
    use agent_contracts::backend::capability::exec::ExecRequest;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let temp = tempfile::tempdir().unwrap();
    let backend = crate::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let path = temp.path().join("external-writer.sh");
    backend
        .files()
        .write_bytes(WriteBytesRequest {
            path: BackendPath::from_raw(path.display().to_string()),
            content: b"#!/bin/sh\nexit 0\n".to_vec(),
            mode: WriteMode::Create,
            extra: None,
        })
        .await
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut writer = process_coordination::spawn_tokio(
        tokio::process::Command::new("/bin/sh")
            .args(["-c", "exec 3>>\"$1\"; echo ready; read line", "writer"])
            .arg(&path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true),
    )
    .await
    .unwrap();
    let mut lines = BufReader::new(writer.stdout.take().unwrap()).lines();
    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("ready"));
    let result = backend
        .exec()
        .exec(ExecRequest {
            command: path.display().to_string(),
            ..Default::default()
        })
        .await;
    writer
        .stdin
        .take()
        .unwrap()
        .write_all(b"release\n")
        .await
        .unwrap();
    writer.wait().await.unwrap();
    assert!(result.unwrap_err().to_string().contains("Text file busy"));
}

#[cfg(unix)]
#[tokio::test]
async fn exec_timeout_includes_creation_group_admission() {
    use agent_contracts::backend::capability::exec::ExecRequest;
    let temp = tempfile::tempdir().unwrap();
    let backend = crate::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let permit = process_coordination::write_permit().await;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        backend.exec().exec(ExecRequest {
            command: "/bin/true".into(),
            timeout_ms: Some(20),
            ..Default::default()
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.timed_out);
    assert_eq!(result.exit_code, None);
    drop(permit);
    assert_eq!(
        backend
            .exec()
            .exec(ExecRequest {
                command: "/bin/true".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .exit_code,
        Some(0)
    );
}
