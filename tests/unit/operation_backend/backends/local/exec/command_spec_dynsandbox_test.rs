use super::*;
use crate::backends::local::backend::LocalBackendState;
use crate::backends::local::exec::LocalExec;
use crate::backends::local::factory::local_backend_with_isolation;
use crate::backends::local::policy::LocalBackendPolicy;
use crate::test_support::test_workspace_root;
use agent_contracts::backend::capability::exec::{ExecResult, LineSink};
use agent_contracts::backend::BackendPath;
use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Interaction handle that answers every dyn-sandbox AUTH prompt with a
/// fixed decision, recording the prompts it was shown for assertions.
struct ScriptedInteraction {
    allow: bool,
    prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl InteractionHandle for ScriptedInteraction {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let prompt = match request {
            InteractionRequest::Choice { prompt, .. } => prompt.clone(),
            _ => String::new(),
        };
        self.prompts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(prompt);
        let value = if self.allow {
            Some("Allow".to_string())
        } else {
            Some("Deny".to_string())
        };
        InteractionResponse::Choice { value }
    }
}

#[test]
fn command_from_spec_builds_linux_dynsandbox_args() {
    let workspace = PathBuf::from("/workspace");
    let policy = LocalBackendPolicy::test_isolated(
        "linux_dynsandbox",
        vec![workspace.clone(), workspace.join("tmp")],
        vec![workspace.join("tmp")],
        false,
    );
    let request = ExecRequest {
        command: "echo hi".to_string(),
        args: vec![],
        shell: Some("bash".to_string()),
        cwd: Some(BackendPath::from_raw("/workspace".to_string())),
        timeout_ms: Some(1_000),
        ..Default::default()
    };
    let command = command_from_spec(
        &request,
        LocalCommandSpec {
            program: "bash".to_string(),
            args: vec!["-c".to_string(), "echo hi".to_string()],
        },
        &policy,
        Some(workspace.as_path()),
    );

    assert_eq!(command.as_std().get_program(), "dyn-sandbox");
    let args: Vec<String> = command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        vec![
            "--mount",
            "/workspace:ro",
            "--mount",
            "/workspace/tmp:rw",
            "-c",
            "/workspace",
            "--",
            "bash",
            "-c",
            "echo hi",
        ]
    );
}

/// Install a fake `dyn-sandbox` script on PATH (once per test process). The
/// real binary is not installed in this environment, and the streaming
/// tests need the backend's `linux_dynsandbox_available()` PATH probe to pass.
/// The script plays the sandbox side of the AUTH protocol: it announces a
/// blocked path on stderr, reads the decision from stdin (the AUTH control
/// channel), and echoes it back so tests can observe what was written.
fn install_fake_linux_dynsandbox() -> &'static PathBuf {
    static FAKE_DYN_SANDBOX: OnceLock<PathBuf> = OnceLock::new();
    FAKE_DYN_SANDBOX.get_or_init(|| {
        let root = std::env::temp_dir().join(format!(
            "xiaoo-fake-dyn-sandbox-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
        std::fs::create_dir_all(root.as_path()).unwrap();
        let bin = root.join("dyn-sandbox");
        std::fs::write(
            bin.as_path(),
            "#!/bin/sh\necho \"AUTH_REQ:shadow:/etc/shadow\" >&2\nIFS= read -r response\necho \"verdict:$response\"\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.as_path(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        let mut paths =
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
        paths.insert(0, root.clone());
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        root
    })
}

fn linux_dynsandbox_backend(
    workspace: PathBuf,
    writable: PathBuf,
) -> std::sync::Arc<dyn agent_contracts::backend::OperationBackend> {
    local_backend_with_isolation(
        workspace.clone(),
        None,
        Some(writable.clone()),
        None,
        Some(json!({
            "kind": "linux_dynsandbox",
            "allow_network": false,
            "readable_roots": [workspace.to_string_lossy().to_string()],
            "writable_roots": [writable.to_string_lossy().to_string()]
        })),
    )
    .unwrap()
}

async fn linux_dynsandbox_exec_bash(
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

fn linux_dynsandbox_exec_with_auth(
    workspace: &Path,
    command: &str,
    allow: bool,
) -> (ExecResult, Vec<String>) {
    let _guard = crate::test_support::process_group_test_lock()
        .lock()
        .unwrap();
    install_fake_linux_dynsandbox();
    let root = test_workspace_root("xiaoo-dyn-sandbox-", "auth");
    let writable = workspace.join("tmp");
    std::fs::create_dir_all(writable.as_path()).unwrap();

    let backend = linux_dynsandbox_backend(workspace.to_path_buf(), writable);
    let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    backend.attach_interaction(Arc::new(ScriptedInteraction {
        allow,
        prompts: prompts.clone(),
    }));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(linux_dynsandbox_exec_bash(
        backend.as_ref(),
        workspace,
        command,
    ));
    let prompts = prompts.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let _ = std::fs::remove_dir_all(root.as_path());
    (result, prompts)
}

#[test]
fn linux_dynsandbox_streaming_allows_blocked_path() {
    let root = test_workspace_root("xiaoo-dyn-sandbox-", "allow");
    let workspace = root.join("workspace");
    let (result, prompts) = linux_dynsandbox_exec_with_auth(&workspace, "cat /etc/shadow", true);

    assert_eq!(result.exit_code, Some(0));
    assert!(
        String::from_utf8_lossy(result.stdout.as_slice()).contains("verdict:ALLOW"),
        "stdout was: {:?}",
        String::from_utf8_lossy(result.stdout.as_slice())
    );
    assert_eq!(prompts.len(), 1, "prompts: {prompts:?}");
    assert!(
        prompts[0].contains("/etc/shadow"),
        "prompt was: {:?}",
        prompts[0]
    );
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn linux_dynsandbox_streaming_denies_on_user_choice() {
    let root = test_workspace_root("xiaoo-dyn-sandbox-", "deny");
    let workspace = root.join("workspace");
    let (result, prompts) = linux_dynsandbox_exec_with_auth(&workspace, "cat /etc/shadow", false);

    assert_eq!(result.exit_code, Some(0));
    assert!(
        String::from_utf8_lossy(result.stdout.as_slice()).contains("verdict:DENY"),
        "stdout was: {:?}",
        String::from_utf8_lossy(result.stdout.as_slice())
    );
    assert_eq!(prompts.len(), 1);
    let _ = std::fs::remove_dir_all(root.as_path());
}

/// Regression test for the UFCS recursion bug in `exec_streaming`:
/// when `policy.requires_stdin()` is true (LinuxDynsandbox
/// isolation), the override used to call
/// `OperationExec::exec_streaming(self, request, sink)` — UFCS that
/// resolves to `<LocalExec as OperationExec>::exec_streaming` (this
/// same override), not the trait's default impl. Result: infinite
/// recursion → stack overflow on every `exec_streaming` call under
/// dyn-sandbox isolation. The existing streaming tests above use
/// `exec()` (not `exec_streaming()`), so they didn't cover the
/// fallback branch.
///
/// With the fix, the override calls `exec_streaming_via_exec`
/// (free function), which delegates to `exec()` (the AUTH-aware
/// override) and feeds buffered stdout through the sink.
#[test]
fn linux_dynsandbox_exec_streaming_does_not_recurse() {
    let _guard = crate::test_support::process_group_test_lock()
        .lock()
        .unwrap();
    install_fake_linux_dynsandbox();
    let root = test_workspace_root("xiaoo-dyn-sandbox-", "stream-norecurse");
    let workspace = root.join("workspace");
    let writable = workspace.join("tmp");
    std::fs::create_dir_all(writable.as_path()).unwrap();

    let backend = linux_dynsandbox_backend(workspace.to_path_buf(), writable);
    let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    backend.attach_interaction(Arc::new(ScriptedInteraction {
        allow: true,
        prompts: prompts.clone(),
    }));

    // Sink that collects every line it sees, never asks to stop.
    // Under the bug, `exec_streaming` would recurse before any line
    // reached the sink — so an empty `seen` + a process exit is the
    // first observable symptom of the stack-overflow path.
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    struct CollectAll {
        seen: Arc<Mutex<Vec<String>>>,
    }
    #[async_trait]
    impl LineSink for CollectAll {
        // LineSink::on_line is sync — no async needed.
        fn on_line(&self, line: &str) -> bool {
            self.seen.lock().unwrap().push(line.to_string());
            true
        }
    }
    let sink: Arc<dyn LineSink> = Arc::new(CollectAll { seen: seen.clone() });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        // Hard ceiling so a re-introduced recursion doesn't hang
        // the test suite forever — the original bug would spin
        // until the OS killed the process for OOM/stack overflow.
        .build()
        .unwrap();
    let result = runtime
        .block_on(backend.exec().exec_streaming(
            ExecRequest {
                command: "cat /etc/shadow".to_string(),
                args: vec![],
                shell: Some("bash".to_string()),
                cwd: Some(BackendPath::from_raw(
                    workspace.to_string_lossy().into_owned(),
                )),
                timeout_ms: Some(5_000),
                ..Default::default()
            },
            sink,
        ))
        .unwrap();

    // The fake dyn-sandbox script echoes "verdict:ALLOW" on stdout
    // after the AUTH handshake. The sink must have seen it — proves
    // we reached the buffer-then-sink fallback instead of recursing.
    let seen_guard = seen.lock().unwrap();
    assert!(
        seen_guard.iter().any(|line| line.contains("verdict:ALLOW")),
        "sink must have received the verdict line; got {seen_guard:?}"
    );

    // Output ownership contract: `exec_streaming_via_exec` clears
    // stdout unconditionally (the sink owns collected state).
    assert!(
        result.stdout.is_empty(),
        "exec_streaming result.stdout must be empty (sink owns state); got {} bytes",
        result.stdout.len()
    );
    assert!(!result.stopped_early, "sink never returned false");
    assert_eq!(result.exit_code, Some(0));

    // AUTH handshake still fired once — proves the fallback used
    // `exec()` (the AUTH-aware override), not some non-AUTH path.
    let prompts_guard = prompts.lock().unwrap();
    assert_eq!(prompts_guard.len(), 1, "AUTH prompt must fire exactly once");

    let _ = std::fs::remove_dir_all(root.as_path());
}

/// Regression test for the auth-channel busy-wait: when stderr_task ends
/// before the child does (EOF on our read side), the main loop must stop
/// polling the closed auth channel so the deadline timer arms and the
/// still-running child is killed on timeout. Without the `auth_open` guard
/// the loop spins on the closed channel, the timeout never fires, and exec
/// only returns once the child exits on its own (~5s later) with no
/// timeout signal.
#[test]
fn exec_linux_dynsandbox_times_out_when_stderr_closes_early() {
    let _guard = crate::test_support::process_group_test_lock()
        .lock()
        .unwrap();

    let exec = LocalExec::new(Arc::new(LocalBackendState {
        backend_id: "test".to_string(),
        workspace_root: BackendPath::from_raw("/workspace".to_string()),
        workspace_root_host: std::env::current_dir().expect("current dir"),
        temp_root_host: std::env::temp_dir(),
        ..Default::default()
    }));

    // Close stderr immediately (EOF on our read side) but keep running well
    // past the 500ms timeout so the deadline is the only thing that can end
    // the exec promptly.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let start = std::time::Instant::now();
    let result = runtime.block_on(async {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("exec 2>&-; sleep 5")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("spawn child");
        let pgid = child.id().unwrap_or(0) as i32;
        if pgid > 0 {
            crate::process_group::register_pgid(pgid);
        }
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        exec.exec_linux_dynsandbox(Some(500), child, stdin, stdout, stderr, pgid)
            .await
            .expect("exec_linux_dynsandbox")
    });
    let elapsed = start.elapsed();

    assert!(
        result.timed_out,
        "expected timeout, got exit_code={:?}",
        result.exit_code
    );
    assert_eq!(result.exit_code, None);
    assert!(
        elapsed < Duration::from_secs(3),
        "exec took {elapsed:?}; without the auth_open guard it spins until `sleep 5` exits"
    );
}

/// Regression test for non-UTF-8 stderr: a stray invalid-UTF-8 line must
/// not kill the stderr reader, or subsequent regular stderr is lost and —
/// worse — a later `AUTH_REQ` is never relayed (the sandbox would block
/// waiting for a decision and only fail via timeout). The line is dropped
/// and reading continues.
#[test]
fn exec_linux_dynsandbox_survives_invalid_utf8_stderr_line() {
    let _guard = crate::test_support::process_group_test_lock()
        .lock()
        .unwrap();

    let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let exec = LocalExec::new(Arc::new(LocalBackendState {
        backend_id: "test".to_string(),
        workspace_root: BackendPath::from_raw("/workspace".to_string()),
        workspace_root_host: std::env::current_dir().expect("current dir"),
        temp_root_host: std::env::temp_dir(),
        interaction: std::sync::RwLock::new(Some(Arc::new(ScriptedInteraction {
            allow: true,
            prompts: prompts.clone(),
        }))),
        ..Default::default()
    }));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(async {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(
                "printf '\\377\\376\\377\\n' >&2; printf 'ok-line\\n' >&2; \
                         echo 'AUTH_REQ:shadow:/etc/shadow' >&2; IFS= read -r resp; \
                         echo \"verdict:$resp\"",
            )
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .expect("spawn child");
        let pgid = child.id().unwrap_or(0) as i32;
        if pgid > 0 {
            crate::process_group::register_pgid(pgid);
        }
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        exec.exec_linux_dynsandbox(Some(5_000), child, stdin, stdout, stderr, pgid)
            .await
            .expect("exec_linux_dynsandbox")
    });

    assert_eq!(result.exit_code, Some(0), "stderr={:?}", result.stderr);
    assert_eq!(
        String::from_utf8_lossy(result.stderr.as_slice()),
        "ok-line\n"
    );
    assert!(
        String::from_utf8_lossy(result.stdout.as_slice()).contains("verdict:ALLOW"),
        "stdout was: {:?}",
        String::from_utf8_lossy(result.stdout.as_slice())
    );
    let prompts = prompts.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(prompts.len(), 1, "prompts: {prompts:?}");
    assert!(
        prompts[0].contains("/etc/shadow"),
        "prompt was: {:?}",
        prompts[0]
    );
}
