use super::{resolve_visible_tool_names, tui_entry_context};
use crate::app_state::AppState;
use crate::config::{AgentRoleConfig, Config};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[tokio::test]
async fn resolve_visible_tool_names_requires_exact_tool_names() {
    let mut config = Config::default();
    config.agent.insert(
        "code-reviewer".to_string(),
        AgentRoleConfig {
            description: String::new(),
            prompt: None,
            max_turns: None,
            tools: BTreeMap::from([
                ("write".to_string(), false),
                ("file_write".to_string(), false),
            ]),
        },
    );

    let mut state =
        AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
    state.active_agent_role = Some("code-reviewer".to_string());

    let visible = resolve_visible_tool_names(&state)
        .await
        .expect("tool visibility should resolve");
    let visible: BTreeSet<_> = visible.into_iter().collect();

    assert!(visible.contains("file_edit"));
    assert!(!visible.contains("file_write"));
}

#[test]
fn tui_entry_context_carries_active_agent_role() {
    let mut config = Config::default();
    config.agent.insert(
        "plan".to_string(),
        AgentRoleConfig {
            description: String::new(),
            prompt: None,
            max_turns: None,
            tools: BTreeMap::new(),
        },
    );
    let mut state =
        AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
    state.active_agent_role = Some("plan".to_string());

    let entry = tui_entry_context(&state, None);

    assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
}

#[test]
fn tui_entry_context_carries_instance_id_for_remote() {
    let config = Config::default();
    let mut state =
        AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
            .expect("app state should initialize");
    state.active_agent_role = Some("plan".to_string());

    let entry = tui_entry_context(&state, Some("http://127.0.0.1:18080".to_string()));

    // Both the role and the remote instance_id must be set so the
    // daemon can resolve [agent.<name>] (max_turns) in remote mode.
    assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
    assert_eq!(entry.instance_id.as_deref(), Some("http://127.0.0.1:18080"));
}
