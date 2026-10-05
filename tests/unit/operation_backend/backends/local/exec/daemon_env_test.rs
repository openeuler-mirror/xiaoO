use super::*;
use crate::backends::local::factory::local_backend_with_isolation;
use crate::test_support::test_workspace_root;
use agent_contracts::backend::BackendPath;

/// daemon 在启动时会用 `std::env::set_var` 把密钥库里的明文凭据写进自己的环境
/// （`inject_llm_secrets_into_env`）。本地执行后端若默认继承父进程环境，
/// 一次 `env` 就能把这些凭据原样打印出来。这里用无沙箱路径（`isolation: None`）
/// 覆盖到那条裸执行的 `Command`。
#[test]
fn exec_does_not_inherit_the_daemon_environment() {
    let root = test_workspace_root("xiaoo-daemon-env-", "exec");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.as_path()).unwrap();

    // 冒充 daemon 环境：一个"凭据"，以及一个请求显式指定的变量。
    std::env::set_var("XIAOO_TEST_DAEMON_SECRET", "sk-must-not-leak");
    std::env::set_var("XIAOO_TEST_REQUESTED", "explicitly-passed");

    let backend =
        local_backend_with_isolation(workspace.clone(), None, None, None, None).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        backend
            .exec()
            .exec(ExecRequest {
                command: "env".to_string(),
                args: vec![],
                shell: Some("bash".to_string()),
                cwd: Some(BackendPath::from_raw(
                    workspace.to_string_lossy().into_owned(),
                )),
                timeout_ms: Some(5_000),
                env: Some(vec![(
                    "XIAOO_TEST_REQUESTED".to_string(),
                    "explicitly-passed".to_string(),
                )]),
                ..Default::default()
            })
            .await
            .unwrap()
    });

    std::env::remove_var("XIAOO_TEST_DAEMON_SECRET");
    std::env::remove_var("XIAOO_TEST_REQUESTED");
    let _ = std::fs::remove_dir_all(root.as_path());

    let stdout = String::from_utf8_lossy(&result.stdout).into_owned();

    assert!(
        !stdout.contains("sk-must-not-leak"),
        "the executed process must not inherit the daemon's environment; it saw:\n{stdout}"
    );
    assert!(
        stdout.contains("XIAOO_TEST_REQUESTED=explicitly-passed"),
        "variables passed on the request must still reach the process; it saw:\n{stdout}"
    );
    assert!(
        stdout.contains("PATH="),
        "PATH has to survive the clear, otherwise the shell cannot even start; it saw:\n{stdout}"
    );
}
