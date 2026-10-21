//! Mutation externalization: a spawn_wrapper with `externalize_fs_writes:
//! true` routes write_bytes / create_dir_all through
//! `[wrapper.prefix...] <cli> request --stdin`. The CLI is stubbed with a
//! python script (records the envelope, performs the write, prints a
//! result) and the wrapper with the same dummy shape used by the exec
//! suite. The stub CLI is injected via `XIAOO_BACKEND_CLI` behind a shared
//! lock (env mutation is process-global; cargo runs tests in parallel
//! threads).

use crate::backends::local::backend::LocalBackendState;
use crate::backends::local::filesystem::LocalFileSystem;
use agent_contracts::backend::capability::filesystem::{
    OperationFileSystem, WriteBytesRequest, WriteMode,
};
use agent_contracts::backend::{BackendPath, OperationError};
use serde_json::json;
use std::sync::{Arc, Mutex, OnceLock};

/// Serializes tests that mutate `XIAOO_BACKEND_CLI`.
fn cli_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Point `XIAOO_BACKEND_CLI` at `cli` for the duration of `body`, under the
/// shared lock (env mutation is process-global; cargo runs tests in
/// parallel threads).
fn with_stub_cli<T>(cli: &std::path::Path, body: impl FnOnce() -> T) -> T {
    let _guard = cli_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("XIAOO_BACKEND_CLI", cli);
    let result = body();
    std::env::remove_var("XIAOO_BACKEND_CLI");
    result
}

fn wrapper_extra(prefix: Vec<String>) -> serde_json::Value {
    json!([
        {
            "hooker_id": "plugin-x",
            "spawn_wrapper": {
                "prefix": prefix,
                "externalize_fs_writes": true
            }
        }
    ])
}

fn state_with_source_config(workspace: &std::path::Path) -> Arc<LocalBackendState> {
    Arc::new(LocalBackendState {
        backend_id: "local-test".to_string(),
        workspace_root: BackendPath::from_raw(workspace.display().to_string()),
        workspace_root_host: workspace.to_path_buf(),
        temp_root_host: workspace.join("tmp"),
        source_config: Some(json!({
            "kind": "local",
            "options": { "workspace_root": workspace.display().to_string() }
        })),
        ..Default::default()
    })
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

/// Dummy wrapper: records its own argv to `argv.txt` beside the script, then
/// execs everything after a `--` separator (or everything when absent).
fn write_dummy_wrapper(dir: &std::path::Path) -> std::path::PathBuf {
    let wrapper = dir.join("dummy-wrapper.sh");
    std::fs::write(
        &wrapper,
        concat!(
            "#!/bin/sh\n",
            "d=$(dirname \"$0\")\n",
            "printf '%s\\n' \"$@\" > \"$d/argv.txt\"\n",
            "n=$#\n",
            "skip=-1\n",
            "i=1\n",
            "while [ $i -le $n ]; do\n",
            "  eval \"a=\\${$i}\"\n",
            "  if [ \"$a\" = \"--\" ]; then skip=$i; break; fi\n",
            "  i=$((i+1))\n",
            "done\n",
            "if [ $skip -ge 0 ]; then\n",
            "  i=0\n",
            "  while [ $i -lt $skip ]; do shift; i=$((i+1)); done\n",
            "fi\n",
            "exec \"$@\"\n",
        ),
    )
    .expect("write wrapper script");
    make_executable(&wrapper);
    wrapper
}

/// Stub backend CLI: records the envelope to `envelope.json` beside itself,
/// then executes the request like the real CLI and prints the result JSON.
fn write_stub_cli(dir: &std::path::Path) -> std::path::PathBuf {
    let cli = dir.join("xiaoo-backend-cli");
    std::fs::write(
        &cli,
        concat!(
            "#!/usr/bin/env python3\n",
            "import base64, json, os, sys\n",
            "envelope = json.load(sys.stdin)\n",
            "here = os.path.dirname(os.path.abspath(__file__))\n",
            "with open(os.path.join(here, 'envelope.json'), 'w') as f:\n",
            "    json.dump(envelope, f)\n",
            "request = envelope['request']\n",
            "kind, args = request['kind'], request['args']\n",
            "if kind == 'write_bytes':\n",
            "    data = base64.b64decode(args['content_b64'])\n",
            "    existed = os.path.exists(args['path'])\n",
            "    parent = os.path.dirname(args['path'])\n",
            "    if parent:\n",
            "        os.makedirs(parent, exist_ok=True)\n",
            "    with open(args['path'], 'wb') as f:\n",
            "        f.write(data)\n",
            "    print(json.dumps({'path': args['path'], 'created': not existed}))\n",
            "elif kind == 'create_dir_all':\n",
            "    os.makedirs(args['path'], exist_ok=True)\n",
            "    print(json.dumps({'path': args['path'], 'created': True}))\n",
            "else:\n",
            "    print(json.dumps({'error': 'unsupported'}))\n",
            "    sys.exit(3)\n",
        ),
    )
    .expect("write stub cli script");
    make_executable(&cli);
    cli
}

fn block_on_write(
    fs: LocalFileSystem,
    request: WriteBytesRequest,
) -> Result<agent_contracts::backend::capability::filesystem::WriteBytesOutcome, OperationError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(fs.write_bytes(request))
}

#[test]
fn externalized_write_bytes_round_trips_through_wrapper_and_cli() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let wrapper = write_dummy_wrapper(temp.path());
    let cli = write_stub_cli(temp.path());

    let fs = LocalFileSystem::new(state_with_source_config(&workspace));
    let target = workspace.join("out").join("data.bin");
    let payload: Vec<u8> = vec![0x00, 0x01, 0xff, 0x10, b'\n', 0x80];
    let outcome = with_stub_cli(&cli, || {
        block_on_write(
            fs,
            WriteBytesRequest {
                path: BackendPath::from_raw(target.display().to_string()),
                content: payload.clone(),
                mode: WriteMode::AtomicOverwrite,
                extra: Some(wrapper_extra(vec![
                    wrapper.display().to_string(),
                    "--session=s1".to_string(),
                    "--".to_string(),
                ])),
            },
        )
    });

    let outcome = outcome.expect("externalized write succeeds");
    assert!(outcome.created, "stub reports a new file");
    assert_eq!(outcome.path.native(), target.display().to_string());
    assert_eq!(
        std::fs::read(&target).expect("file written by the stub CLI"),
        payload,
        "binary payload must round-trip through the envelope"
    );

    // The wrapper saw: its own flags + the CLI invocation.
    let argv = std::fs::read_to_string(temp.path().join("argv.txt")).unwrap();
    assert_eq!(
        argv.trim_end(),
        format!("--session=s1\n--\n{}\nrequest\n--stdin", cli.display())
    );

    let envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(temp.path().join("envelope.json")).unwrap())
            .unwrap();
    assert_eq!(
        envelope["backend"],
        json!({ "kind": "local", "options": { "workspace_root": workspace.display().to_string() } })
    );
    assert_eq!(envelope["request"]["kind"], "write_bytes");
    assert_eq!(
        envelope["request"]["args"]["path"],
        target.display().to_string()
    );
    assert_eq!(envelope["request"]["args"]["mode"], "atomic_overwrite");
    let decoded: Vec<u8> = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(envelope["request"]["args"]["content_b64"].as_str().unwrap())
            .unwrap()
    };
    assert_eq!(decoded, payload);
}

#[test]
fn externalized_create_dir_all_routes_to_subprocess() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let wrapper = write_dummy_wrapper(temp.path());
    let cli = write_stub_cli(temp.path());

    let fs = LocalFileSystem::new(state_with_source_config(&workspace));
    let target = workspace.join("nested/dir");
    let result = with_stub_cli(&cli, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(fs.create_dir_all_with_extra(
                &BackendPath::from_raw(target.display().to_string()),
                Some(&wrapper_extra(vec![wrapper.display().to_string()])),
            ))
    });

    result.expect("externalized mkdir succeeds");
    assert!(target.is_dir());
    // Envelope field fidelity is pinned by the write_bytes suite test; here
    // only the routing signal matters.
    let envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(temp.path().join("envelope.json")).unwrap())
            .unwrap();
    assert_eq!(envelope["request"]["kind"], "create_dir_all");
}

#[test]
fn externalized_request_error_fails_loud() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    // A CLI that always fails with a stderr message.
    let cli = temp.path().join("xiaoo-backend-cli");
    std::fs::write(
        &cli,
        "#!/bin/sh\ncat > /dev/null\necho 'policy denied the write' >&2\nexit 4\n",
    )
    .unwrap();
    make_executable(&cli);
    let wrapper = write_dummy_wrapper(temp.path());

    let fs = LocalFileSystem::new(state_with_source_config(&workspace));
    let error = with_stub_cli(&cli, || {
        block_on_write(
            fs,
            WriteBytesRequest {
                path: BackendPath::from_raw(workspace.join("x.txt").display().to_string()),
                content: b"data".to_vec(),
                mode: WriteMode::Overwrite,
                extra: Some(wrapper_extra(vec![wrapper.display().to_string()])),
            },
        )
    });

    let error = error.expect_err("subprocess failure must surface");
    match error {
        OperationError::ExecutionFailed { message } => {
            assert!(
                message.contains("policy denied the write"),
                "stderr must surface: {message}"
            );
        }
        other => panic!("expected ExecutionFailed, got {other:?}"),
    }
}

#[test]
fn missing_source_config_with_instruction_fails_loud() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let wrapper = write_dummy_wrapper(temp.path());

    // Ad-hoc state without an archived source config.
    let state = Arc::new(LocalBackendState {
        backend_id: "local-test".to_string(),
        workspace_root: BackendPath::from_raw(workspace.display().to_string()),
        workspace_root_host: workspace.clone(),
        temp_root_host: workspace.join("tmp"),
        ..Default::default()
    });
    let fs = LocalFileSystem::new(state);

    let error = block_on_write(
        fs,
        WriteBytesRequest {
            path: BackendPath::from_raw(workspace.join("x.txt").display().to_string()),
            content: b"data".to_vec(),
            mode: WriteMode::Overwrite,
            extra: Some(wrapper_extra(vec![wrapper.display().to_string()])),
        },
    )
    .expect_err("no source config must fail loud");

    assert!(matches!(error, OperationError::ExecutionFailed { .. }));
    assert!(
        !workspace.join("x.txt").exists(),
        "no in-process fallback write"
    );
}

#[test]
fn writes_without_the_externalize_instruction_stay_in_process() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let state = state_with_source_config(&workspace);
    // A wrapper without externalize_fs_writes (it only affects exec) and no
    // extra at all: both must stay on the in-process write path.
    let extras: [Option<serde_json::Value>; 2] = [
        Some(json!([
            { "hooker_id": "plugin-x", "spawn_wrapper": { "prefix": ["/nonexistent/wrapper"] } }
        ])),
        None,
    ];
    for (index, extra) in extras.into_iter().enumerate() {
        let target = workspace.join(format!("in-process-{index}.txt"));
        std::fs::write(&target, "old").unwrap();

        let outcome = block_on_write(
            LocalFileSystem::new(Arc::clone(&state)),
            WriteBytesRequest {
                path: BackendPath::from_raw(target.display().to_string()),
                content: b"plain".to_vec(),
                mode: WriteMode::Overwrite,
                extra,
            },
        )
        .expect("in-process write");

        assert!(!outcome.created, "the file already existed");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "plain");
    }
}

#[test]
fn externalized_write_timeout_kills_subprocess() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    // A wrapper that hangs forever; the caller's timeout must kill the
    // whole process group.
    let wrapper = temp.path().join("hang-wrapper.sh");
    std::fs::write(&wrapper, "#!/bin/sh\nsleep 300\n").unwrap();
    make_executable(&wrapper);
    let cli = write_stub_cli(temp.path());

    let fs = LocalFileSystem::new(state_with_source_config(&workspace));
    let start = std::time::Instant::now();
    let result = with_stub_cli(&cli, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    fs.write_bytes(WriteBytesRequest {
                        path: BackendPath::from_raw(
                            workspace.join("never.txt").display().to_string(),
                        ),
                        content: b"data".to_vec(),
                        mode: WriteMode::Overwrite,
                        extra: Some(wrapper_extra(vec![wrapper.display().to_string()])),
                    }),
                )
                .await
            })
    });

    assert!(result.is_err(), "the hanging wrapper must trip the timeout");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "timeout must fire promptly, not wait for the wrapper"
    );
}

/// V-X4, end to end through the real factory: a programmatically built
/// backend with isolation archives its config into source_config; a backend
/// rebuilt from the envelope (what the real CLI does) enforces the same
/// isolation policy. Sync `#[test]` + `block_on` so the env-var guard never
/// crosses an await (clippy await_holding_lock).
#[test]
#[cfg(unix)]
fn policy_is_preserved_when_rebuilding_from_source_config() {
    use agent_contracts::backend::{OperationBackendBuildInput, OperationBackendBuilder};

    #[cfg(target_os = "linux")]
    install_fake_dynsandbox();

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sandbox_root = temp.path().join("sandbox");
    std::fs::create_dir_all(&sandbox_root).unwrap();

    #[cfg(target_os = "linux")]
    let isolation = json!({
        "kind": "linux_dynsandbox",
        "allow_network": false,
        "readable_roots": [workspace.display().to_string()],
        "writable_roots": [sandbox_root.display().to_string()]
    });
    #[cfg(not(target_os = "linux"))]
    let isolation = json!({
        "kind": "macos_seatbelt",
        "allow_network": false,
        "readable_roots": [workspace.display().to_string()],
        "writable_roots": [sandbox_root.display().to_string()]
    });

    let backend = crate::backends::local::local_backend_with_isolation(
        workspace.clone(),
        None,
        Some(std::env::temp_dir()),
        None,
        Some(isolation.clone()),
    )
    .expect("isolated local backend");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Externalize an in-root write so the stub CLI captures the envelope's
    // backend section (in-root success also shows the isolation is not
    // over-restrictive).
    let wrapper = write_dummy_wrapper(temp.path());
    let cli = write_stub_cli(temp.path());
    with_stub_cli(&cli, || {
        runtime.block_on(backend.files().write_bytes(WriteBytesRequest {
            path: BackendPath::from_raw(sandbox_root.join("ok.txt").display().to_string()),
            content: b"ok".to_vec(),
            mode: WriteMode::Overwrite,
            extra: Some(wrapper_extra(vec![wrapper.display().to_string()])),
        }))
    })
    .expect("in-root externalized write succeeds");

    // No policy downgrade in the externalized path.
    let envelope: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(temp.path().join("envelope.json")).unwrap())
            .unwrap();
    assert_eq!(envelope["backend"]["kind"], "local");
    assert_eq!(envelope["backend"]["options"]["isolation"], isolation);

    // Rebuild from the envelope's backend section — the same policy must
    // keep holding; the out-of-root denial is the no-downgrade signal.
    let builder = crate::OperationBackendBuilderImpl::new();
    let rebuilt = runtime
        .block_on(builder.build(&OperationBackendBuildInput {
            config: Some(agent_contracts::backend::OperationBackendConfig::new(
                envelope["backend"]["kind"].as_str().unwrap().to_string(),
                envelope["backend"]["options"].clone(),
            )),
            ..OperationBackendBuildInput::default()
        }))
        .expect("rebuild from envelope backend section");

    let denial = runtime
        .block_on(rebuilt.files().write_bytes(WriteBytesRequest {
            path: BackendPath::from_raw(workspace.join("denied.txt").display().to_string()),
            content: b"nope".to_vec(),
            mode: WriteMode::Overwrite,
            extra: None,
        }))
        .expect_err("out-of-root write denied in subprocess mode");
    assert!(matches!(denial, OperationError::SandboxPolicyDenied { .. }));
}

/// One-time fake `dyn-sandbox` on PATH so `linux_dynsandbox` isolation can
/// be built in tests (the availability probe only needs the binary to exist).
#[cfg(target_os = "linux")]
fn install_fake_dynsandbox() -> &'static std::path::PathBuf {
    use std::sync::OnceLock;
    static FAKE: OnceLock<std::path::PathBuf> = OnceLock::new();
    FAKE.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "xiaoo-externalize-fake-dyn-sandbox-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let bin = root.join("dyn-sandbox");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&bin);
        let mut paths = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .collect::<Vec<_>>();
        paths.insert(0, root.clone());
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        root
    })
}
