use super::*;
use agent_contracts::backend::{
    SandboxPermissionGrantRequest, SandboxPermissionScope, SandboxPolicyDenial,
};

fn policy() -> LocalBackendPolicy {
    LocalBackendPolicy::from_isolation_options(
        Some(LocalIsolationOptions::MacosSeatbelt {
            allow_network: false,
            readable_roots: vec!["/workspace".to_string()],
            writable_roots: vec!["/workspace/tmp".to_string()],
        }),
        Path::new("/workspace"),
        Path::new("/tmp"),
    )
    .unwrap_or_else(|_| LocalBackendPolicy {
        isolation: LocalIsolationConfig::MacosSeatbelt(PathIsolationConfig {
            allow_network: false,
            readable_roots: vec![PathBuf::from("/workspace"), PathBuf::from("/workspace/tmp")],
            writable_roots: vec![PathBuf::from("/workspace/tmp")],
        }),
        grants: Arc::new(Mutex::new(GrantStore::default())),
    })
}

#[test]
fn workspace_read_is_allowed() {
    assert!(policy()
        .check_read(Path::new("/workspace/src/lib.rs"), "file_read")
        .is_ok());
}

#[test]
fn outside_workspace_read_is_denied() {
    assert!(matches!(
        policy().check_read(Path::new("/etc/passwd"), "file_read"),
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
}

#[test]
fn escaped_path_is_denied_after_normalization() {
    assert!(matches!(
        policy().check_read(Path::new("/workspace/../etc/passwd"), "file_read"),
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
}

#[test]
fn writable_root_is_also_readable() {
    assert!(policy()
        .check_read(Path::new("/workspace/tmp/result.txt"), "file_read")
        .is_ok());
    assert!(policy()
        .check_write(Path::new("/workspace/tmp/result.txt"), "file_write")
        .is_ok());
}

#[test]
fn write_outside_writable_root_is_denied() {
    assert!(matches!(
        policy().check_write(Path::new("/workspace/src/lib.rs"), "file_write"),
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
}

#[test]
fn sandbox_denial_for_read_path_requires_existing_host_path() {
    let root = std::env::temp_dir().join(format!(
        "xiaoo-local-policy-read-denial-{}",
        std::process::id()
    ));
    let outside = root.join("outside");
    std::fs::create_dir_all(outside.as_path()).unwrap();
    let file = outside.join("secret.txt");
    std::fs::write(file.as_path(), b"secret").unwrap();
    let policy = LocalBackendPolicy::test_isolated(
        "linux_bubblewrap",
        vec![root.join("workspace")],
        vec![root.join("workspace")],
        true,
    );

    let denial = policy
        .sandbox_denial_for_path(file.as_path(), SandboxPermissionCapability::Read, "bash")
        .unwrap()
        .expect("outside existing file should be denied");
    assert_eq!(denial.isolation, "linux_bubblewrap");
    assert_eq!(denial.capability, SandboxPermissionCapability::Read);

    assert!(policy
        .sandbox_denial_for_path(
            outside.join("missing.txt").as_path(),
            SandboxPermissionCapability::Read,
            "bash",
        )
        .unwrap()
        .is_none());
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn sandbox_denial_for_write_path_allows_missing_file_with_existing_parent() {
    let root = std::env::temp_dir().join(format!(
        "xiaoo-local-policy-write-denial-{}",
        std::process::id()
    ));
    let outside = root.join("outside");
    std::fs::create_dir_all(outside.as_path()).unwrap();
    let policy = LocalBackendPolicy::test_isolated(
        "linux_bubblewrap",
        vec![root.join("workspace")],
        vec![root.join("workspace")],
        true,
    );

    let denial = policy
        .sandbox_denial_for_path(
            outside.join("new.txt").as_path(),
            SandboxPermissionCapability::Write,
            "bash",
        )
        .unwrap()
        .expect("new file under existing outside directory should be denied");
    assert_eq!(denial.isolation, "linux_bubblewrap");
    assert_eq!(denial.capability, SandboxPermissionCapability::Write);

    assert!(policy
        .sandbox_denial_for_path(
            outside.join("missing-parent").join("new.txt").as_path(),
            SandboxPermissionCapability::Write,
            "bash",
        )
        .unwrap()
        .is_none());
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn sandbox_denial_for_path_skips_already_allowed_roots() {
    let root = std::env::temp_dir().join(format!(
        "xiaoo-local-policy-allowed-denial-{}",
        std::process::id()
    ));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.as_path()).unwrap();
    let workspace = std::fs::canonicalize(workspace.as_path()).unwrap();
    let file = workspace.join("visible.txt");
    std::fs::write(file.as_path(), b"visible").unwrap();
    let policy = LocalBackendPolicy::test_isolated(
        "linux_bubblewrap",
        vec![workspace.clone()],
        vec![workspace],
        true,
    );

    assert!(policy
        .sandbox_denial_for_path(file.as_path(), SandboxPermissionCapability::Read, "bash")
        .unwrap()
        .is_none());
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn granted_read_root_is_allowed() {
    let granted_policy = policy();
    let request = grant_request(
        SandboxPermissionCapability::Read,
        "/outside",
        SandboxPermissionScope::Session,
    );

    granted_policy.grant(request).unwrap();

    assert!(policy()
        .check_read(Path::new("/outside/secret.txt"), "file_read")
        .is_err());
    assert!(granted_policy
        .check_read(Path::new("/outside/secret.txt"), "file_read")
        .is_ok());
}

#[test]
fn granted_write_root_is_also_readable_and_revocable() {
    let policy = policy();
    let id = policy
        .grant(grant_request(
            SandboxPermissionCapability::Write,
            "/outside/result.txt",
            SandboxPermissionScope::Once,
        ))
        .unwrap();

    assert!(policy
        .check_write(Path::new("/outside/result.txt"), "file_write")
        .is_ok());
    assert!(policy
        .check_read(Path::new("/outside/result.txt"), "file_read")
        .is_ok());

    policy.revoke(id).unwrap();

    assert!(matches!(
        policy.check_write(Path::new("/outside/result.txt"), "file_write"),
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
}

#[test]
fn granted_write_path_allows_read_write_and_atomic_temp() {
    let root = std::env::temp_dir().join(format!("xiaoo-local-policy-rw-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(root.as_path());
    std::fs::create_dir_all(root.as_path()).unwrap();
    let file = root.join("runtime.txt");
    std::fs::write(file.as_path(), b"runtime").unwrap();
    let policy = policy();
    let id = policy
        .grant(grant_request(
            SandboxPermissionCapability::Write,
            file.to_string_lossy().as_ref(),
            SandboxPermissionScope::Once,
        ))
        .unwrap();

    assert!(policy.check_read(file.as_path(), "bash").is_ok());
    assert!(policy.check_write(file.as_path(), "bash").is_ok());
    assert!(policy
        .check_write(root.join(".runtime.txt.tmp").as_path(), "file_write")
        .is_ok());

    policy.revoke(id).unwrap();
    let _ = std::fs::remove_dir_all(root.as_path());
}

#[test]
fn profile_includes_granted_roots() {
    let policy = policy();
    policy
        .grant(grant_request(
            SandboxPermissionCapability::Read,
            "/granted",
            SandboxPermissionScope::Session,
        ))
        .unwrap();

    let text = policy.seatbelt_profile().unwrap().to_profile_text();

    assert!(text.contains("\"/granted\""));
}

#[test]
fn exec_runtime_grant_disables_seatbelt_profile_for_exec() {
    let policy = policy();
    assert!(policy.seatbelt_profile().is_some());

    policy
        .grant(grant_request(
            SandboxPermissionCapability::ExecRuntime,
            "<runtime path not reported by macOS Seatbelt>",
            SandboxPermissionScope::Once,
        ))
        .unwrap();

    assert!(policy.seatbelt_profile().is_none());
}

#[test]
fn exec_runtime_session_grant_is_rejected() {
    let policy = policy();

    let result = policy.grant(grant_request(
        SandboxPermissionCapability::ExecRuntime,
        "<runtime path not reported by macOS Seatbelt>",
        SandboxPermissionScope::Session,
    ));

    assert!(matches!(result, Err(OperationError::Unsupported { .. })));
    assert!(policy.seatbelt_profile().is_some());
}

#[test]
fn profile_includes_roots_and_omits_network_when_disabled() {
    let text = policy().seatbelt_profile().unwrap().to_profile_text();
    assert!(text.contains("\"/workspace\""));
    assert!(text.contains("\"/workspace/tmp\""));
    assert!(!text.contains("(allow network*)"));
}

#[test]
fn macos_seatbelt_defaults_to_allowing_network() {
    let options: LocalIsolationOptions =
        serde_json::from_value(serde_json::json!({"kind": "macos_seatbelt"})).unwrap();
    let LocalIsolationOptions::MacosSeatbelt { allow_network, .. } = options else {
        panic!("expected macos seatbelt options");
    };

    assert!(allow_network);
}

#[test]
fn linux_bubblewrap_defaults_to_allowing_network() {
    let options: LocalIsolationOptions =
        serde_json::from_value(serde_json::json!({"kind": "linux_bubblewrap"})).unwrap();
    let LocalIsolationOptions::LinuxBubblewrap { allow_network, .. } = options else {
        panic!("expected linux bubblewrap options");
    };

    assert!(allow_network);
}

#[test]
fn bubblewrap_args_bind_configured_roots_and_disable_network() {
    let policy = LocalBackendPolicy::test_isolated(
        "linux_bubblewrap",
        vec![
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace/.tmp"),
        ],
        vec![PathBuf::from("/workspace/.tmp")],
        false,
    );

    let args = policy.bubblewrap_args(Path::new("/workspace")).unwrap();

    assert!(args.iter().any(|arg| arg == "--unshare-net"));
    assert!(args
        .windows(3)
        .any(|window| window == ["--ro-bind", "/workspace", "/workspace"]));
    assert!(args
        .windows(3)
        .any(|window| window == ["--bind", "/workspace/.tmp", "/workspace/.tmp"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--chdir", "/workspace"]));
}

#[test]
fn exec_runtime_grant_disables_bubblewrap_args_for_exec() {
    let policy = LocalBackendPolicy::test_isolated(
        "linux_bubblewrap",
        vec![PathBuf::from("/workspace")],
        vec![PathBuf::from("/workspace")],
        true,
    );
    assert!(policy.bubblewrap_args(Path::new("/workspace")).is_some());

    policy
        .grant(grant_request(
            SandboxPermissionCapability::ExecRuntime,
            "<runtime path not reported by local sandbox>",
            SandboxPermissionScope::Once,
        ))
        .unwrap();

    assert!(policy.bubblewrap_args(Path::new("/workspace")).is_none());
}

#[test]
fn bubblewrap_denial_reports_linux_isolation() {
    let policy = LocalBackendPolicy::test_isolated(
        "linux_bubblewrap",
        vec![PathBuf::from("/workspace")],
        vec![PathBuf::from("/workspace/tmp")],
        true,
    );

    let error = policy
        .check_write(Path::new("/workspace/src/lib.rs"), "file_write")
        .unwrap_err();

    let OperationError::SandboxPolicyDenied { denial } = error else {
        panic!("expected sandbox denial");
    };
    assert_eq!(denial.isolation, "linux_bubblewrap");
}

fn grant_request(
    capability: SandboxPermissionCapability,
    path: &str,
    scope: SandboxPermissionScope,
) -> SandboxPermissionGrantRequest {
    SandboxPermissionGrantRequest {
        denial: SandboxPolicyDenial {
            backend_id: "local".to_string(),
            isolation: "macos_seatbelt".to_string(),
            operation: "test".to_string(),
            capability,
            path: path.to_string(),
        },
        scope,
    }
}

#[cfg(not(target_os = "macos"))]
#[test]
fn macos_seatbelt_is_unsupported_on_non_macos() {
    let error = LocalBackendPolicy::from_isolation_options(
        Some(LocalIsolationOptions::MacosSeatbelt {
            allow_network: false,
            readable_roots: vec![],
            writable_roots: vec![],
        }),
        Path::new("/workspace"),
        Path::new("/tmp"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        OperationBackendBuildError::Unsupported { .. }
    ));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn linux_bubblewrap_is_unsupported_on_non_linux() {
    let error = LocalBackendPolicy::from_isolation_options(
        Some(LocalIsolationOptions::LinuxBubblewrap {
            allow_network: false,
            readable_roots: vec![],
            writable_roots: vec![],
        }),
        Path::new("/workspace"),
        Path::new("/tmp"),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        OperationBackendBuildError::Unsupported { .. }
    ));
}

#[test]
fn linux_dynsandbox_config_deserializes_new_format() {
    // Same serde path as config.toml `[operation_backend.options.isolation]`.
    let options: LocalIsolationOptions =
        serde_json::from_value(serde_json::json!({"kind": "linux_dynsandbox"})).unwrap();
    let LocalIsolationOptions::LinuxDynsandbox {
        allow_network,
        no_landlock,
        readable_roots,
        writable_roots,
    } = options
    else {
        panic!("expected dyn sandbox options");
    };

    assert!(allow_network);
    // no_landlock defaults to true (landlock disabled) at this stage.
    assert!(no_landlock);
    assert!(readable_roots.is_empty());
    assert!(writable_roots.is_empty());
}

#[test]
fn linux_dynsandbox_args_bind_configured_roots() {
    let policy = LocalBackendPolicy::test_isolated(
        "linux_dynsandbox",
        vec![
            PathBuf::from("/workspace"),
            PathBuf::from("/workspace/.tmp"),
        ],
        vec![PathBuf::from("/workspace/.tmp")],
        false,
    );

    let args = policy
        .linux_dynsandbox_args(Path::new("/workspace"), None)
        .unwrap();

    assert!(args
        .windows(2)
        .any(|window| window == ["--mount", "/workspace:ro"]));
    assert!(args
        .windows(2)
        .any(|window| window == ["--mount", "/workspace/.tmp:rw"]));
    assert!(args.windows(2).any(|window| window == ["-c", "/workspace"]));
    // test_isolated constructs LinuxDynsandbox with no_landlock=false.
    assert!(!args.iter().any(|arg| arg == "--no-landlock"));
}

#[test]
fn linux_dynsandbox_checks_enforce_configured_roots() {
    let policy = LocalBackendPolicy::test_isolated(
        "linux_dynsandbox",
        vec![PathBuf::from("/workspace")],
        vec![PathBuf::from("/workspace/.tmp")],
        false,
    );

    // Within configured roots -> allowed.
    assert!(policy
        .check_write(Path::new("/workspace/.tmp/out.txt"), "file_write")
        .is_ok());
    assert!(policy
        .check_read(Path::new("/workspace/src/lib.rs"), "file_read")
        .is_ok());
    // Outside configured roots -> denied (reuses bwrap's path checks).
    let error = policy
        .check_read(Path::new("/etc/passwd"), "file_read")
        .unwrap_err();
    let OperationError::SandboxPolicyDenied { denial } = error else {
        panic!("expected sandbox denial");
    };
    assert_eq!(denial.isolation, "linux_dynsandbox");
}

impl super::LocalBackendPolicy {
    pub(crate) fn test_isolated(
        isolation_name: &str,
        readable_roots: Vec<PathBuf>,
        writable_roots: Vec<PathBuf>,
        allow_network: bool,
    ) -> Self {
        let config = PathIsolationConfig {
            allow_network,
            readable_roots,
            writable_roots,
        };
        let isolation = match isolation_name {
            "linux_bubblewrap" => LocalIsolationConfig::LinuxBubblewrap(config),
            "linux_dynsandbox" => LocalIsolationConfig::LinuxDynsandbox(LinuxDynsandboxOptions {
                roots: config.clone(),
            }),
            _ => LocalIsolationConfig::MacosSeatbelt(config),
        };
        Self {
            isolation,
            grants: Arc::new(Mutex::new(GrantStore::default())),
        }
    }
}

impl super::LocalBackendPolicy {
    pub(crate) fn test_macos_seatbelt(
        readable_roots: Vec<PathBuf>,
        writable_roots: Vec<PathBuf>,
        allow_network: bool,
    ) -> Self {
        Self::test_isolated(
            "macos_seatbelt",
            readable_roots,
            writable_roots,
            allow_network,
        )
    }
}
