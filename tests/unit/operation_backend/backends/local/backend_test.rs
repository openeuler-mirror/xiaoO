use super::*;

fn test_state(home_dir_host: Option<PathBuf>) -> LocalBackendState {
    let workspace_root_host = std::env::current_dir().expect("current dir");
    let workspace_root = BackendPath::from_raw(workspace_root_host.to_string_lossy().into_owned());
    let home_dir = home_dir_host
        .as_ref()
        .map(|path| BackendPath::from_raw(path.to_string_lossy().into_owned()));

    LocalBackendState {
        backend_id: "test".to_string(),
        workspace_root,
        workspace_root_host,
        home_dir,
        home_dir_host,
        temp_root_host: std::env::temp_dir(),
        ..Default::default()
    }
}

#[test]
fn resolves_tilde_paths_against_home_dir() {
    let home = std::env::current_dir().expect("current dir").join("home");
    let state = test_state(Some(home.clone()));
    let resolved = state
        .resolve_host_path("~/.xiaoo/tools/md_to_html.mjs", Path::new("/workspace"))
        .expect("resolve");

    assert_eq!(resolved, home.join(".xiaoo/tools/md_to_html.mjs"));
}

#[test]
fn tilde_requires_configured_home_dir() {
    let state = test_state(None);
    let error = state
        .resolve_host_path("~/missing", Path::new("/workspace"))
        .expect_err("tilde should require home");

    assert!(matches!(error, OperationError::Unsupported { .. }));
}
