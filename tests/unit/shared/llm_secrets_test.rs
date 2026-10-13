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

#[test]
fn empty_secret_store_uses_configured_environment_without_rewriting_file() {
    let temp_dir = tempfile::TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("config.toml");
    let env_name = "XIAOO_TEST_EMPTY_STORE_API_KEY";
    let secret_path = llm_secrets_path(&config_path);
    std::fs::write(&secret_path, []).expect("write intentional empty store");
    std::env::set_var(env_name, "environment-only-test-secret");
    inject_llm_secrets_into_env(&config_path).expect("empty store has no secrets to inject");
    assert_eq!(
        get_llm_secret(&config_path, env_name).expect("resolve environment reference"),
        "environment-only-test-secret"
    );
    let metadata = inspect_secret_store(&config_path).expect("inspect empty store");
    assert!(metadata.api_keys.is_empty());
    assert!(metadata.tokens.is_empty());
    assert_eq!(std::fs::metadata(&secret_path).unwrap().len(), 0);
    std::env::remove_var(env_name);
}

#[test]
fn empty_secret_store_without_environment_does_not_supply_a_credential() {
    let temp_dir = tempfile::TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("config.toml");
    let env_name = "XIAOO_TEST_EMPTY_STORE_MISSING_KEY";
    std::env::remove_var(env_name);
    std::fs::write(llm_secrets_path(&config_path), []).expect("write intentional empty store");
    let error = get_llm_secret(&config_path, env_name).expect_err("no credential is available");
    assert!(error.to_string().contains("not found"));
}

#[test]
fn nonempty_malformed_secret_store_does_not_fall_back_to_environment() {
    let temp_dir = tempfile::TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("config.toml");
    let env_name = "XIAOO_TEST_MALFORMED_STORE_API_KEY";
    std::env::set_var(env_name, "must-not-mask-malformed-store");
    let path = llm_secrets_path(&config_path);
    for bytes in [b"x".as_slice(), &[0u8; 64][..]] {
        std::fs::write(&path, bytes).expect("write malformed store");
        assert!(get_llm_secret(&config_path, env_name).is_err());
        assert!(inject_llm_secrets_into_env(&config_path).is_err());
        assert!(inspect_secret_store(&config_path).is_err());
    }
    std::env::remove_var(env_name);
}
