use super::*;
use agent_contracts::backend::{
    capability::{
        exec::ExecRequest,
        filesystem::{WriteBytesRequest, WriteMode},
    },
    BackendPath,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[cfg(unix)]
async fn write_while_implicit_creator_runs(output: bool) {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let backend =
        operation_backend::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let path = temp.path().join("product-executable.sh");
    let stop = Arc::new(AtomicBool::new(false));
    let producer = std::thread::spawn({
        let stop = Arc::clone(&stop);
        move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    for _ in 0..512 {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        if output {
                            assert!(tokio::task::spawn_blocking(|| output_blocking(
                                &mut std::process::Command::new("/bin/true")
                            ))
                            .await
                            .unwrap()
                            .unwrap()
                            .status
                            .success());
                        } else {
                            assert!(status(&mut std::process::Command::new("/bin/true"))
                                .await
                                .unwrap()
                                .success());
                        }
                    }
                })
        }
    });
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
    println!("WRITE_RETURNED implicit_output={output}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let result = backend
        .exec()
        .exec(ExecRequest {
            command: path.display().to_string(),
            ..Default::default()
        })
        .await;
    stop.store(true, Ordering::Release);
    producer.join().unwrap();
    assert_eq!(result.unwrap().exit_code, Some(0));
}

#[cfg(unix)]
#[tokio::test]
async fn executable_write_excludes_implicit_status_creation() {
    write_while_implicit_creator_runs(false).await;
}

#[cfg(unix)]
#[tokio::test]
async fn executable_write_excludes_implicit_output_creation() {
    write_while_implicit_creator_runs(true).await;
}
