//! X1c: the plugin-provided `spawn_wrapper.prefix` must be prepended to the
//! tool subprocess argv — outermost — leaving every original token intact,
//! and the argv must be byte-identical to the current behavior when no
//! instruction is present. Assertions run against the `Command` produced by
//! `command_from_spec` (same style as the bubblewrap/dynsandbox suites), plus
//! an end-to-end run through `LocalExec` with a dummy wrapper script that
//! records its argv and then execs the real command.

use super::*;
use crate::backends::local::backend::LocalBackendState;
use crate::backends::local::exec::LocalExec;
use agent_contracts::backend::capability::OperationExec;
use agent_contracts::backend::BackendPath;
use serde_json::json;
use std::sync::Arc;

fn argv_of(command: &Command) -> Vec<String> {
    command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn program_of(command: &Command) -> String {
    command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned()
}

fn plain_spec() -> LocalCommandSpec {
    LocalCommandSpec {
        program: "/bin/echo".to_string(),
        args: vec!["hello".to_string(), "world".to_string()],
    }
}

fn wrapper_extra(prefix: &[&str]) -> serde_json::Value {
    serde_json::json!([
        {
            "hooker_id": "plugin-x",
            "spawn_wrapper": { "prefix": prefix }
        }
    ])
}

#[test]
fn prefix_is_prepended_outermost_and_tokens_are_preserved() {
    let request = ExecRequest {
        command: "/bin/echo".to_string(),
        args: vec!["hello".to_string(), "world".to_string()],
        extra: Some(wrapper_extra(&["/opt/wrap", "--session=s1", "--"])),
        ..Default::default()
    };
    let command = command_from_spec(&request, plain_spec(), &Default::default(), None);

    assert_eq!(program_of(&command), "/opt/wrap");
    assert_eq!(
        argv_of(&command),
        vec!["--session=s1", "--", "/bin/echo", "hello", "world"]
    );
}

#[test]
fn no_instruction_yields_the_plain_command_line() {
    let request = ExecRequest {
        command: "/bin/echo".to_string(),
        args: vec!["hello".to_string(), "world".to_string()],
        ..Default::default()
    };
    let command = command_from_spec(&request, plain_spec(), &Default::default(), None);

    assert_eq!(program_of(&command), "/bin/echo");
    assert_eq!(argv_of(&command), vec!["hello", "world"]);
}

#[test]
fn shell_spec_is_wrapped_as_a_whole() {
    // bash executor shape: shell=Some, command carries the script.
    let request = ExecRequest {
        command: "echo hi".to_string(),
        shell: Some("bash".to_string()),
        extra: Some(wrapper_extra(&["/opt/wrap", "--"])),
        ..Default::default()
    };
    let spec = LocalCommandSpec {
        program: "bash".to_string(),
        args: vec!["-c".to_string(), "echo hi".to_string()],
    };
    let command = command_from_spec(&request, spec, &Default::default(), None);

    assert_eq!(program_of(&command), "/opt/wrap");
    assert_eq!(argv_of(&command), vec!["--", "bash", "-c", "echo hi"]);
}

#[test]
fn extra_without_spawn_wrapper_keeps_plain_command_line() {
    let request = ExecRequest {
        command: "/bin/echo".to_string(),
        args: vec!["hi".to_string()],
        extra: Some(json!([{ "hooker_id": "plugin-x", "other": 1 }])),
        ..Default::default()
    };
    let command = command_from_spec(&request, plain_spec(), &Default::default(), None);

    assert_eq!(program_of(&command), "/bin/echo");
    assert_eq!(argv_of(&command), vec!["hello", "world"]);
}

#[cfg(unix)]
fn local_exec_state(workspace: &std::path::Path) -> Arc<LocalBackendState> {
    Arc::new(LocalBackendState {
        backend_id: "local-test".to_string(),
        workspace_root: BackendPath::from_raw(workspace.display().to_string()),
        workspace_root_host: workspace.to_path_buf(),
        temp_root_host: workspace.join("tmp"),
        ..Default::default()
    })
}

/// Dummy wrapper: records its own argv to $RECORD_FILE (one token per line),
/// then execs everything after a `--` separator (or everything, when no
/// separator is present) — the same shape the plan's test helper prescribes
/// ("记录 argv 到临时文件后 exec 真命令"), mirroring how a real wrapper
/// consumes its own flags before running the wrapped command.
#[cfg(unix)]
fn write_dummy_wrapper(dir: &std::path::Path) -> std::path::PathBuf {
    let wrapper = dir.join("dummy-wrapper.sh");
    // A separate writer keeps this harness's concurrent forks from inheriting
    // a writable script descriptor and making another test's exec fail ETXTBSY.
    let status = std::process::Command::new("/bin/sh")
        .args(["-c", "printf '%s' \"$1\" > \"$2\"", "write-wrapper"])
        .arg(concat!(
            "#!/bin/sh\n",
            "printf '%s\\n' \"$@\" > \"$RECORD_FILE\"\n",
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
        ))
        .arg(&wrapper)
        .status()
        .expect("run wrapper script writer");
    assert!(status.success(), "write wrapper script");
    make_executable(&wrapper);
    wrapper
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn run_with_dummy_wrapper(
    workspace: &std::path::Path,
    prefix: Vec<String>,
    record_file: &std::path::Path,
) -> (
    agent_contracts::backend::capability::exec::ExecResult,
    Vec<String>,
) {
    let exec = LocalExec::new(local_exec_state(workspace));
    let request = ExecRequest {
        command: "/bin/echo".to_string(),
        args: vec!["token with spaces".to_string(), "second".to_string()],
        extra: Some(json!([
            { "hooker_id": "plugin-x", "spawn_wrapper": { "prefix": prefix } }
        ])),
        env: Some(vec![(
            "RECORD_FILE".to_string(),
            record_file.display().to_string(),
        )]),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(exec.exec(request)).expect("exec ok");
    let recorded = std::fs::read_to_string(record_file)
        .expect("wrapper recorded argv")
        .lines()
        .map(str::to_string)
        .collect();
    (result, recorded)
}

#[cfg(unix)]
#[test]
fn dummy_wrapper_sees_prefix_and_executes_real_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let wrapper = write_dummy_wrapper(temp.path());
    let record = temp.path().join("argv.txt");

    let (result, recorded) = run_with_dummy_wrapper(
        &workspace,
        vec![
            wrapper.display().to_string(),
            "--session=s1".to_string(),
            "--".to_string(),
        ],
        &record,
    );

    // The wrapper observed its own arguments: prefix tail + the real
    // command, tokens intact (argv[0], the wrapper path itself, is excluded
    // by "$@").
    assert_eq!(
        recorded,
        vec![
            "--session=s1".to_string(),
            "--".to_string(),
            "/bin/echo".to_string(),
            "token with spaces".to_string(),
            "second".to_string(),
        ]
    );
    // The real command ran and its output passed through untouched.
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(
        String::from_utf8_lossy(&result.stdout),
        "token with spaces second\n"
    );
}

#[cfg(unix)]
#[test]
fn dummy_wrapper_propagates_nonzero_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let wrapper = write_dummy_wrapper(temp.path());
    let record = temp.path().join("argv.txt");

    let exec = LocalExec::new(local_exec_state(&workspace));
    let request = ExecRequest {
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), "exit 7".to_string()],
        extra: Some(json!([
            { "hooker_id": "plugin-x", "spawn_wrapper": { "prefix": [wrapper.display().to_string()] } }
        ])),
        env: Some(vec![(
            "RECORD_FILE".to_string(),
            record.display().to_string(),
        )]),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(exec.exec(request)).expect("exec ok");

    assert_eq!(
        result.exit_code,
        Some(7),
        "exit code must propagate through the wrapper"
    );
}

#[cfg(unix)]
#[test]
fn dummy_wrappers_remain_executable_during_concurrent_spawns() {
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..256 {
                let status = std::process::Command::new("true")
                    .status()
                    .expect("spawn concurrent command");
                assert!(status.success());
            }
        });
        for _ in 0..64 {
            dummy_wrapper_propagates_nonzero_exit_code();
        }
    });
}

#[cfg(unix)]
#[test]
fn timeout_kills_the_wrapped_process_group() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    // A wrapper that never execs: simulates a hung wrapper process tree.
    let wrapper = temp.path().join("hang-wrapper.sh");
    std::fs::write(&wrapper, "#!/bin/sh\nsleep 300\n").unwrap();
    make_executable(&wrapper);

    let exec = LocalExec::new(local_exec_state(&workspace));
    let request = ExecRequest {
        command: "/bin/echo".to_string(),
        args: vec!["hi".to_string()],
        timeout_ms: Some(500),
        extra: Some(json!([
            { "hooker_id": "plugin-x", "spawn_wrapper": { "prefix": [wrapper.display().to_string()] } }
        ])),
        ..Default::default()
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime
        .block_on(exec.exec(request))
        .expect("exec completes via timeout path");

    assert!(
        result.timed_out,
        "the wrapped command must be killed on timeout"
    );
}
