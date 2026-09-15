use super::*;

struct UnusedInteraction;

#[async_trait]
impl InteractionHandle for UnusedInteraction {
    async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
        panic!("interaction is not used when reading the default shell")
    }
}

#[test]
fn permission_aware_exec_forwards_default_shell() {
    let root = tempfile::tempdir().expect("tempdir");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let inner =
        operation_backend::local_backend(workspace, None, None, Some("/bin/bash".to_string()))
            .expect("local backend");
    let wrapped = PermissionAwareOperationBackend::new(inner, Arc::new(UnusedInteraction), None);

    assert_eq!(wrapped.exec().default_shell(), Some("/bin/bash"));
}

#[test]
fn parses_cat_runtime_denial_path() {
    let stderr = "cat: /Users/me/.ssh/config: Operation not permitted\n";

    assert_eq!(
        denied_path_from_stderr(stderr).as_deref(),
        Some("/Users/me/.ssh/config")
    );
}

#[test]
fn parses_shell_redirection_runtime_denial_path() {
    let stderr = "bash: /Users/me/out.txt: Operation not permitted\n";

    assert_eq!(
        denied_path_from_stderr(stderr).as_deref(),
        Some("/Users/me/out.txt")
    );
}

#[test]
fn runtime_denial_uses_write_grant_to_cover_unknown_path_access() {
    let result = ExecResult {
        stdout: vec![],
        stderr: b"cat: /Users/me/.ssh/config: Operation not permitted\n".to_vec(),
        exit_code: Some(1),
        timed_out: false,
        ..Default::default()
    };

    assert_eq!(
        runtime_seatbelt_denial(&result).map(|denial| denial.capability),
        Some(SandboxPermissionCapability::Write)
    );
}

#[test]
fn bubblewrap_candidates_parse_read_path_from_no_such_file() {
    let candidates = bubblewrap_path_candidates_from_stderr(
        "cat: /data/secret.txt: No such file or directory\n",
        "cat /data/secret.txt",
    );

    assert_eq!(
        candidates,
        vec![RuntimePathCandidate {
            path: "/data/secret.txt".to_string(),
            capability: SandboxPermissionCapability::Read,
        }]
    );
}

#[test]
fn bubblewrap_candidates_parse_write_path_from_redirection() {
    let candidates = bubblewrap_path_candidates_from_stderr(
        "bash: /data/out.txt: No such file or directory\n",
        "printf hi > /data/out.txt",
    );

    assert_eq!(
        candidates,
        vec![RuntimePathCandidate {
            path: "/data/out.txt".to_string(),
            capability: SandboxPermissionCapability::Write,
        }]
    );
}

#[test]
fn bubblewrap_candidates_parse_write_path_from_pipeline_tool_stderr() {
    let candidates = bubblewrap_path_candidates_from_stderr(
        "tee: /data/out.txt: No such file or directory\n",
        "printf hi | tee /data/out.txt",
    );

    assert_eq!(
        candidates,
        vec![RuntimePathCandidate {
            path: "/data/out.txt".to_string(),
            capability: SandboxPermissionCapability::Write,
        }]
    );
}

#[test]
fn bubblewrap_candidates_parse_quoted_unicode_path() {
    let candidates = bubblewrap_path_candidates_from_stderr(
        "ls: cannot access ‘/data/input folder’: No such file or directory\n",
        "ls '/data/input folder'",
    );

    assert_eq!(
        candidates,
        vec![RuntimePathCandidate {
            path: "/data/input folder".to_string(),
            capability: SandboxPermissionCapability::Read,
        }]
    );
}

#[test]
fn runtime_bubblewrap_denial_requires_permission_control_confirmation() {
    let control = TestPermissionControl {
        expected_path: "/data/secret.txt",
        expected_capability: SandboxPermissionCapability::Read,
        denial: Some(SandboxPolicyDenial {
            backend_id: "local".to_string(),
            isolation: "linux_bubblewrap".to_string(),
            operation: "bash".to_string(),
            capability: SandboxPermissionCapability::Read,
            path: "/data/secret.txt".to_string(),
        }),
    };
    let result = ExecResult {
        stdout: vec![],
        stderr: b"cat: /data/secret.txt: No such file or directory\n".to_vec(),
        exit_code: Some(1),
        timed_out: false,
        ..Default::default()
    };

    let denial = runtime_bubblewrap_denial(Some(&control), "cat /data/secret.txt", &result)
        .expect("expected denial");

    assert_eq!(denial.isolation, "linux_bubblewrap");
    assert_eq!(denial.capability, SandboxPermissionCapability::Read);
}

#[test]
fn runtime_bubblewrap_denial_skips_unconfirmed_paths() {
    let control = TestPermissionControl {
        expected_path: "/missing/path.txt",
        expected_capability: SandboxPermissionCapability::Read,
        denial: None,
    };
    let result = ExecResult {
        stdout: vec![],
        stderr: b"cat: /missing/path.txt: No such file or directory\n".to_vec(),
        exit_code: Some(1),
        timed_out: false,
        ..Default::default()
    };

    assert!(runtime_bubblewrap_denial(Some(&control), "cat /missing/path.txt", &result).is_none());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn bubblewrap_exec_prompts_and_retries_for_existing_hidden_read_path() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let temp_root = workspace.join("tmp");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(temp_root.as_path()).unwrap();
    std::fs::create_dir_all(outside.as_path()).unwrap();
    let secret = outside.join("secret.txt");
    std::fs::write(secret.as_path(), b"secret").unwrap();

    let inner = match operation_backend::local_backend_with_isolation(
        workspace.clone(),
        None,
        Some(temp_root),
        Some("/bin/bash".to_string()),
        Some(serde_json::json!({
            "kind": "linux_bubblewrap",
            "readable_roots": [workspace],
            "writable_roots": [workspace]
        })),
    ) {
        Ok(backend) => backend,
        Err(agent_contracts::backend::OperationBackendBuildError::Unsupported { .. }) => {
            return;
        }
        Err(error) => panic!("backend should build: {error}"),
    };
    let wrapped = PermissionAwareOperationBackend::new(
        inner,
        Arc::new(AllowSessionInteraction),
        Some("linux_bubblewrap"),
    );
    assert_eq!(wrapped.exec().default_shell(), Some("/bin/bash"));

    let result = wrapped
        .exec()
        .exec(ExecRequest {
            command: format!("cat {}", shell_quote(secret.as_path())),
            args: vec![],
            shell: Some("/bin/bash".to_string()),
            cwd: Some(BackendPath::from_raw(workspace.display().to_string())),
            timeout_ms: Some(5000),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert_eq!(String::from_utf8_lossy(result.stdout.as_slice()), "secret");
}

#[test]
fn detects_silent_exec_denial() {
    let result = ExecResult {
        stdout: vec![],
        stderr: vec![],
        exit_code: None,
        timed_out: false,
        ..Default::default()
    };

    assert!(is_silent_seatbelt_exec_denial(&result));
}

#[test]
fn silent_runtime_denial_requests_exec_runtime() {
    let denial = silent_runtime_denial("local");

    assert_eq!(denial.capability, SandboxPermissionCapability::ExecRuntime);
    assert_eq!(denial.path, "<runtime path not reported by macOS Seatbelt>");
}

#[test]
fn exec_runtime_denial_only_offers_once_or_deny() {
    let denial = silent_runtime_denial("local");

    assert_eq!(
        permission_options(&denial, None),
        vec![ALLOW_ONCE.to_string(), DENY.to_string()]
    );
}

#[test]
fn permission_prompt_uses_bubblewrap_display_name() {
    let denial = SandboxPolicyDenial {
        backend_id: "local".to_string(),
        isolation: "linux_bubblewrap".to_string(),
        operation: "file_write".to_string(),
        capability: SandboxPermissionCapability::Write,
        path: "/workspace/src/lib.rs".to_string(),
    };

    let prompt = permission_prompt(&denial, None);

    assert!(prompt.starts_with("Linux Bubblewrap blocked"));
    assert!(!prompt.contains("macOS Seatbelt"));
}

#[test]
fn derives_bash_prefix_from_denied_path_position() {
    assert_eq!(
        bash_command_prefix_for_denial("cat \"/Users/me/a.txt\"", "/Users/me/a.txt").as_deref(),
        Some("cat \"")
    );
}

#[test]
fn bash_rule_matches_prefix_and_parent_path() {
    let rules = BashSandboxApprovalRules::default();
    let denial = SandboxPolicyDenial {
        backend_id: "local".to_string(),
        isolation: "macos_seatbelt".to_string(),
        operation: "bash".to_string(),
        capability: SandboxPermissionCapability::Write,
        path: "/Users/me/docs/a.txt".to_string(),
    };

    rules.add("cat \"/Users/me/docs/a.txt\"", &denial);

    let next_denial = SandboxPolicyDenial {
        path: "/Users/me/docs/b.txt".to_string(),
        ..denial
    };
    assert!(rules.matches("cat \"/Users/me/docs/b.txt\"", &next_denial));
    assert!(!rules.matches("sed -n '1p' \"/Users/me/docs/b.txt\"", &next_denial));
}

#[test]
fn does_not_treat_timeout_as_silent_denial() {
    let result = ExecResult {
        stdout: vec![],
        stderr: vec![],
        exit_code: None,
        timed_out: true,
        ..Default::default()
    };

    assert!(!is_silent_seatbelt_exec_denial(&result));
}

struct TestPermissionControl {
    expected_path: &'static str,
    expected_capability: SandboxPermissionCapability,
    denial: Option<SandboxPolicyDenial>,
}

impl OperationPermissionControl for TestPermissionControl {
    fn sandbox_denial_for_path(
        &self,
        path: &BackendPath,
        capability: SandboxPermissionCapability,
        operation: &str,
    ) -> Result<Option<SandboxPolicyDenial>, OperationError> {
        assert_eq!(path.native(), self.expected_path);
        assert_eq!(capability, self.expected_capability);
        assert_eq!(operation, "bash");
        Ok(self.denial.clone())
    }

    fn grant(
        &self,
        _request: SandboxPermissionGrantRequest,
    ) -> Result<SandboxPermissionGrantId, OperationError> {
        panic!("grant should not be called by runtime_bubblewrap_denial")
    }

    fn revoke(&self, _id: SandboxPermissionGrantId) -> Result<(), OperationError> {
        panic!("revoke should not be called by runtime_bubblewrap_denial")
    }
}

#[cfg(target_os = "linux")]
struct AllowSessionInteraction;

#[cfg(target_os = "linux")]
#[async_trait]
impl InteractionHandle for AllowSessionInteraction {
    async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
        InteractionResponse::Choice {
            value: Some(ALLOW_SESSION.to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}
