use super::{
    delete_llm_secret, get_llm_secret, inject_llm_secrets_into_env, inspect_secret_store,
    llm_secrets_path, save_llm_secret,
};

#[test]
fn saved_secret_can_be_retrieved_on_demand() {
    let temp_dir = tempfile::TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("config.toml");
    let env_name = "XIAOO_TEST_DEEPSEEK_API_KEY";

    std::env::remove_var(env_name);
    std::env::set_var("USE_SDF", "false");
    save_llm_secret(&config_path, env_name, "test-secret").expect("save secret");

    assert_eq!(
        get_llm_secret(&config_path, env_name).expect("resolve selected config secret"),
        "test-secret"
    );
    inject_llm_secrets_into_env(&config_path).expect("inject secret");
    assert_eq!(std::env::var(env_name).as_deref(), Ok("test-secret"));

    let metadata = inspect_secret_store(&config_path).expect("inspect secret store");
    assert_eq!(metadata.api_keys, vec![env_name.to_string()]);
    assert!(metadata.tokens.is_empty());

    std::env::remove_var(env_name);
    std::env::remove_var("USE_SDF");
}

#[test]
fn deleting_the_last_secret_removes_the_store() {
    let temp_dir = tempfile::TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("config.toml");
    save_llm_secret(&config_path, "DELETE_ME", "secret").expect("save secret");
    assert!(delete_llm_secret(&config_path, "DELETE_ME").expect("delete secret"));
    assert!(!llm_secrets_path(&config_path).exists());
    assert!(!delete_llm_secret(&config_path, "DELETE_ME").expect("delete missing secret"));
}
