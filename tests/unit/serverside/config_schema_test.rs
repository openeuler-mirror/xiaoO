use super::config_schema;

#[test]
fn schema_covers_primary_configuration_domains() {
    let schema = config_schema();
    let sections = schema["sections"]
        .as_array()
        .expect("sections should be an array");
    let paths = sections
        .iter()
        .filter_map(|section| section["path"].as_str())
        .collect::<Vec<_>>();

    for required in [
        "llm.models[]",
        "llm.profiles.*",
        "agent.*",
        "subagent.*",
        "skills",
        "hooker",
        "mcp.servers[]",
        "mcp_server",
        "lsp",
        "compact",
        "memory_automation",
        "server.operation_backend",
        "cron",
        "channels.feishu",
        "channels.telegram",
        "trace",
        "vault",
    ] {
        assert!(
            paths.contains(&required),
            "missing schema section {required}"
        );
    }
}

/// `llm.models[]` mirrors the profile fields.
#[test]
fn llm_models_schema_marks_provider_and_model_required() {
    let schema = config_schema();
    let section = schema["sections"]
        .as_array()
        .expect("sections should be an array")
        .iter()
        .find(|section| section["path"] == "llm.models[]")
        .expect("llm.models[] section");
    let fields = section["fields"].as_array().expect("fields array");

    let field = |name: &str| {
        fields
            .iter()
            .find(|field| field["name"] == name)
            .unwrap_or_else(|| panic!("missing field {name}"))
            .clone()
    };
    assert_eq!(field("provider")["required"], true);
    assert_eq!(field("model")["required"], true);
    assert_eq!(field("api_base")["required"], false);
    assert_eq!(field("api_key_env")["type"], "secret_env");
    assert_eq!(field("api_key_env")["secret"], true);
}
