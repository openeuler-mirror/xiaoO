use super::*;

#[cfg(unix)]
#[tokio::test]
async fn executable_write_excludes_lsp_process_creation() {
    use agent_contracts::backend::{
        capability::{
            exec::ExecRequest,
            filesystem::{WriteBytesRequest, WriteMode},
        },
        BackendPath,
    };
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    let temp = tempfile::tempdir().unwrap();
    let backend =
        operation_backend::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let path = temp.path().join("product-executable.sh");
    let stop = Arc::new(AtomicBool::new(false));
    let producer = std::thread::spawn({
        let stop = Arc::clone(&stop);
        let backend = Arc::clone(&backend);
        let cwd = temp.path().to_path_buf();
        move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let lsp = LocalLspEnv::new(backend);
                    for _ in 0..512 {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let mut child = lsp
                            .spawn_process("/bin/true", &[], &cwd)
                            .await
                            .unwrap()
                            .child;
                        assert!(child.wait().await.unwrap().success());
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
    println!("WRITE_RETURNED lsp creator");
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
