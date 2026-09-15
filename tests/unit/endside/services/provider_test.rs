use super::*;
use tempfile::TempDir;

fn temp_state() -> (TempDir, AppState) {
    let temp_dir = TempDir::new().expect("create temp dir");
    let config_path = temp_dir.path().join("nested").join("config.toml");
    let workspace = temp_dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("create workspace");
    let state = AppState::new(config_path, workspace).expect("create app state");
    (temp_dir, state)
}

#[test]
fn persist_active_provider_selection_writes_llm_to_app_config_path() {
    let (_temp_dir, mut state) = temp_state();
    let config_path = state.config_path.clone();

    persist_active_provider_selection(
        &mut state,
        "openai".to_string(),
        "gpt-4o".to_string(),
        Some("OPENAI_API_KEY".to_string()),
        "https://api.openai.com/v1".to_string(),
    );

    let saved = Config::load_from(&config_path).expect("load saved config");
    assert_eq!(saved.llm.provider, "openai");
    assert_eq!(saved.llm.model, "gpt-4o");
    assert_eq!(saved.llm.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
    assert_eq!(saved.llm.api_base, "https://api.openai.com/v1");
}

#[test]
fn persisted_selection_settings_switching_provider_uses_new_provider_defaults() {
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.model = "gpt-4o".to_string();
    state.agent_config.llm.api_key_env = Some("OPENAI_API_KEY".to_string());
    state.agent_config.llm.api_base = "https://api.openai.com/v1".to_string();

    let (api_key_env, api_base) = persisted_selection_settings(&state.agent_config, "deepseek");

    assert_eq!(api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(api_base, "https://api.deepseek.com/v1");
}

#[test]
fn persisted_selection_settings_same_provider_preserves_existing_config() {
    let (_temp_dir, mut state) = temp_state();
    state.agent_config.llm.provider = "openai".to_string();
    state.agent_config.llm.model = "gpt-4o".to_string();
    state.agent_config.llm.api_key_env = Some("CUSTOM_OPENAI_KEY".to_string());
    state.agent_config.llm.api_base = "https://proxy.example/v1".to_string();

    let (api_key_env, api_base) = persisted_selection_settings(&state.agent_config, "openai");

    assert_eq!(api_key_env.as_deref(), Some("CUSTOM_OPENAI_KEY"));
    assert_eq!(api_base, "https://proxy.example/v1");
}

#[test]
fn default_api_base_for_openai_is_explicit() {
    assert_eq!(
        default_api_base_for_provider("openai"),
        "https://api.openai.com/v1"
    );
}

#[test]
fn coding_plan_provider_defaults_are_explicit() {
    assert_eq!(
        default_api_key_env_for_provider("minimax").as_deref(),
        Some("MINIMAX_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("minimax"),
        "https://api.minimaxi.com/v1"
    );
    assert_eq!(
        default_api_key_env_for_provider("kimi").as_deref(),
        Some("MOONSHOT_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("kimi"),
        "https://api.moonshot.cn/v1"
    );
    assert_eq!(
        default_api_key_env_for_provider("minimax-coding-plan").as_deref(),
        Some("MINIMAX_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("minimax-coding-plan"),
        "https://api.minimax.io/v1"
    );
    assert_eq!(
        default_api_key_env_for_provider("kimi-coding-plan").as_deref(),
        Some("KIMI_API_KEY")
    );
    assert_eq!(
        default_api_base_for_provider("kimi-coding-plan"),
        "https://api.kimi.com/coding/v1"
    );
}
