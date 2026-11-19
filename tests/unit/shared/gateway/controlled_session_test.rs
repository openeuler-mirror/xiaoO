use super::*;

#[tokio::test]
async fn controlled_open_requires_a_bound_backend() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let credential = temp.path().join("key");
    std::fs::write(&credential, "test-key").unwrap();
    let profile = ControlledModelProfile {
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        api_base: Some("http://127.0.0.1:1/v1/chat/completions".to_string()),
        api_key_env: None,
        api_key_file: Some(credential),
        context_window: None,
    };

    let error = ControlledTextSession::open_with_backend_manager(
        "unbound".to_string(),
        workspace.clone(),
        &profile,
        Arc::new(BackendManager::new_controlled()),
    )
    .await
    .err()
    .expect("a controlled session cannot open without a binding");

    assert!(matches!(
        error.downcast_ref::<crate::gateway::SessionServiceError>(),
        Some(crate::gateway::SessionServiceError::RuntimeBuild { message })
            if message == "controlled session has no operation backend binding"
    ));
    assert_eq!(std::fs::read_dir(workspace).unwrap().count(), 0);
}
