use super::*;

struct TestClient {
    allow_terminal: bool,
}

#[async_trait]
impl ClientOperations for TestClient {
    async fn read_text(&self, _: &str) -> Result<String, OperationError> {
        panic!("unexpected file read");
    }

    async fn write_text(&self, _: &str, _: String) -> Result<(), OperationError> {
        panic!("unexpected file write");
    }

    async fn run_terminal(&self, command: ClientCommand) -> Result<ExecResult, OperationError> {
        assert!(self.allow_terminal, "unexpected terminal callback");
        let output = tokio::process::Command::new(command.command)
            .args(command.args)
            .current_dir(command.cwd)
            .output()
            .await
            .unwrap();
        let mut stdout = output.stdout;
        stdout.extend(output.stderr);
        let truncated = stdout.len() > command.output_byte_limit as usize;
        stdout.truncate(command.output_byte_limit as usize);
        Ok(ExecResult {
            stdout,
            exit_code: output.status.code(),
            output: ExecOutput::Combined { truncated },
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn glob_filters_remote_listing_before_the_output_limit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let noise = "x".repeat(192);
    for i in 0..400 {
        std::fs::File::create(root.join(format!("{noise}-{i}"))).unwrap();
    }
    std::fs::create_dir(root.join("nested")).unwrap();
    for path in ["keep1.txt", "nested/keep2.txt", "keep9.txt"] {
        std::fs::File::create(root.join(path)).unwrap();
    }
    let backend = ClientBackend::new(
        "client".into(),
        root,
        ClientOperationCapabilities {
            terminal: true,
            ..Default::default()
        },
        Arc::new(TestClient {
            allow_terminal: true,
        }),
    )
    .unwrap();

    let files = backend
        .glob(GlobRequest {
            pattern: "**/keep[12].txt".into(),
            base_dir: None,
            limit: Some(2),
        })
        .await
        .unwrap();
    let paths: Vec<_> = files
        .iter()
        .map(|path| path.relative_path.as_str())
        .collect();
    assert_eq!(paths, ["keep1.txt", "nested/keep2.txt"]);
}

#[tokio::test]
async fn incomplete_file_capabilities_issue_no_callbacks() {
    let read_only = ClientBackend::new(
        "read-only".into(),
        Path::new("/workspace"),
        ClientOperationCapabilities {
            read: true,
            ..Default::default()
        },
        Arc::new(TestClient {
            allow_terminal: false,
        }),
    )
    .unwrap();
    let path = BackendPath::from_raw("/workspace/file.txt");
    assert!(matches!(
        read_only
            .read_bytes(ReadBytesRequest { path: path.clone() })
            .await,
        Err(OperationError::Unsupported { .. })
    ));

    let write_without_read = ClientBackend::new(
        "write-without-read".into(),
        Path::new("/workspace"),
        ClientOperationCapabilities {
            read: false,
            write: true,
            terminal: true,
        },
        Arc::new(TestClient {
            allow_terminal: false,
        }),
    )
    .unwrap();
    assert!(matches!(
        write_without_read
            .write_bytes(WriteBytesRequest {
                path: path.clone(),
                content: b"example".to_vec(),
                mode: WriteMode::Overwrite,
                extra: None,
            })
            .await,
        Err(OperationError::Unsupported { .. })
    ));
    assert!(matches!(
        write_without_read.create_dir_all(&path).await,
        Err(OperationError::Unsupported { .. })
    ));
}
