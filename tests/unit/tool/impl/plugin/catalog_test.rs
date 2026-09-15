use super::{
    declarative_tool_catalog, render_declarative_tool, test_declarative_tool, DeclarativeToolDraft,
    DeclarativeToolDraftEffect,
};
use serde_json::json;
use std::fs;

#[test]
fn reports_invalid_shadowed_and_unsupported_manifests() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    let home = temp.path().join("home");
    let workspace_tools = workspace.join(".xiaoo/tools");
    let home_tools = home.join(".xiaoo/tools");
    fs::create_dir_all(&workspace_tools).expect("workspace tools");
    fs::create_dir_all(&home_tools).expect("home tools");
    let manifest = |description: &str| {
        format!(
        "name = \"echo\"\ndescription = \"{description}\"\n[input_schema]\ntype = \"object\"\n[exec]\ncommand = \"missing-catalog-command\"\n"
    )
    };
    fs::write(workspace_tools.join("echo.toml"), manifest("workspace"))
        .expect("workspace manifest");
    fs::write(home_tools.join("echo.toml"), manifest("global")).expect("global manifest");
    fs::write(home_tools.join("invalid.toml"), "name = [").expect("invalid manifest");

    let catalog = declarative_tool_catalog(Some(&workspace), Some(&home), false);
    assert!(!catalog.supported);
    assert_eq!(catalog.tools.len(), 3);
    assert!(catalog
        .tools
        .iter()
        .all(|tool| tool.content_hash.len() == 64));
    assert_eq!(catalog.tools[0].status, "unsupported_backend");
    assert!(catalog.tools.iter().any(|tool| tool.status == "shadowed"));
    assert!(catalog.tools.iter().any(|tool| tool.status == "invalid"));
}

#[test]
fn renders_and_reloads_a_valid_manifest() {
    let report = render_declarative_tool(DeclarativeToolDraft {
        name: "echo_payload".to_string(),
        description: "Echo input".to_string(),
        timeout_ms: 5_000,
        input_schema: json!({"type": "object", "required": ["message"]}),
        output_description: Some("Echo result".to_string()),
        command: "sh".to_string(),
        args: vec!["./echo.sh".to_string()],
        stdin: "json".to_string(),
        stdout: "text".to_string(),
        env_names: vec!["TOKEN".to_string()],
        effect: DeclarativeToolDraftEffect {
            side_effects: true,
            ..DeclarativeToolDraftEffect::default()
        },
    });
    assert!(report.valid);
    let content = report.content.expect("rendered content");
    let parsed: toml::Value = toml::from_str(&content).expect("valid TOML");
    assert_eq!(parsed["name"].as_str(), Some("echo_payload"));
    assert_eq!(parsed["exec"]["stdin"].as_str(), Some("json"));
    assert_eq!(parsed["effect"]["side_effects"].as_bool(), Some(true));
}

#[test]
fn rejects_invalid_visual_drafts() {
    let report = render_declarative_tool(DeclarativeToolDraft {
        name: "bad name".to_string(),
        description: String::new(),
        timeout_ms: 0,
        input_schema: json!([]),
        output_description: None,
        command: String::new(),
        args: Vec::new(),
        stdin: "binary".to_string(),
        stdout: "text".to_string(),
        env_names: Vec::new(),
        effect: DeclarativeToolDraftEffect::default(),
    });
    assert!(!report.valid);
    assert!(report.content.is_none());
    assert!(report.error.is_some());
}

#[tokio::test]
async fn tests_only_active_tools_and_requires_effect_confirmation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    let tools = workspace.join(".xiaoo/tools");
    fs::create_dir_all(&tools).expect("tool dir");
    let path = tools.join("echo.toml");
    fs::write(
        &path,
        "name = \"echo\"\ndescription = \"echo\"\n[input_schema]\ntype = \"object\"\n[exec]\ncommand = \"sh\"\nargs = [\"-c\", \"printf tested\"]\nstdin = \"none\"\n",
    )
    .expect("manifest");
    let report = test_declarative_tool(&workspace, None, &path, json!({}), true, false).await;
    assert!(report.success);
    assert_eq!(report.output.as_deref(), Some("tested"));

    fs::write(
        &path,
        "name = \"echo\"\ndescription = \"echo\"\n[input_schema]\ntype = \"object\"\n[effect]\nside_effects = true\n[exec]\ncommand = \"sh\"\nargs = [\"-c\", \"printf tested\"]\nstdin = \"none\"\n",
    )
    .expect("effect manifest");
    let denied = test_declarative_tool(&workspace, None, &path, json!({}), true, false).await;
    assert!(!denied.success);
    assert!(denied
        .error
        .as_deref()
        .is_some_and(|error| error.contains("confirmation")));
}
