use super::*;
use crate::backends::local::backend::LocalBackendState;
use crate::backends::local::policy::LocalBackendPolicy;
use agent_contracts::backend::BackendPath;

fn export(root: &std::path::Path) -> LocalExport {
    let workspace = root.join("workspace");
    let temp = workspace.join("tmp");
    std::fs::create_dir_all(temp.as_path()).unwrap();
    LocalExport::new(Arc::new(LocalBackendState {
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
fn export_outside_policy_root_is_denied() {
    let root =
        std::env::temp_dir().join(format!("xiaoo-local-export-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(root.as_path());
    std::fs::create_dir_all(root.join("workspace")).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();
    let file = root.join("outside").join("secret.txt");
    std::fs::write(file.as_path(), b"secret").unwrap();
    let export = export(root.as_path());

    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(export.export_file(ExportFileRequest {
            path: BackendPath::from_raw(file.display().to_string()),
            preferred_name: None,
        }));

    assert!(matches!(
        result,
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
    let _ = std::fs::remove_dir_all(root.as_path());
}
