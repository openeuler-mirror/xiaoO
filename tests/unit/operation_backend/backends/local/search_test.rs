use super::*;
use crate::backends::local::backend::LocalBackendState;
use crate::backends::local::policy::LocalBackendPolicy;

fn search(root: &Path) -> LocalSearch {
    let workspace = root.join("workspace");
    let temp = workspace.join("tmp");
    std::fs::create_dir_all(temp.as_path()).unwrap();
    LocalSearch::new(Arc::new(LocalBackendState {
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
fn grep_outside_policy_root_is_denied() {
    let root =
        std::env::temp_dir().join(format!("xiaoo-local-search-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(root.as_path());
    std::fs::create_dir_all(root.join("workspace")).unwrap();
    std::fs::create_dir_all(root.join("outside")).unwrap();
    let search = search(root.as_path());

    let result = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(search.grep(GrepRequest {
            base_dir: BackendPath::from_raw(root.join("outside").display().to_string()),
            query: "secret".to_string(),
            mode: GrepMode::Content,
            include: None,
            head_limit: None,
        }));

    assert!(matches!(
        result,
        Err(OperationError::SandboxPolicyDenied { .. })
    ));
    let _ = std::fs::remove_dir_all(root.as_path());
}
