use super::role_catalog;
use tempfile::TempDir;

#[test]
fn lists_effective_agent_and_subagent_roles() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[llm]
active_profile = "local"

[llm.profiles.local]
provider = "ollama"
model = "qwen3"

[agent.reviewer]
description = "Review code"
prompt = "Review carefully"
max_turns = 6

[agent.reviewer.tools]
file_write = false

[subagent.researcher]
description = "Research topics"

[subagent.researcher.tools]
web_search = true
"#,
    )
    .expect("write config");

    let report = role_catalog(&config_path).expect("role catalog");
    let reviewer = report
        .agent_roles
        .iter()
        .find(|role| role.id == "reviewer")
        .expect("reviewer role");
    assert!(!reviewer.builtin);
    assert_eq!(reviewer.max_turns, Some(6));
    assert_eq!(reviewer.tools.get("file_write"), Some(&false));
    assert!(report
        .agent_roles
        .iter()
        .any(|role| role.id == "plan" && role.builtin));
    assert!(report
        .subagent_roles
        .iter()
        .any(|role| role.id == "researcher" && !role.builtin));
}
