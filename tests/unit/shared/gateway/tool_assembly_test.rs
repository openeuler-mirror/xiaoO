use super::{discover_tools, ToolAssemblyInput, ToolCatalogSource};

#[tokio::test]
async fn catalog_exposes_builtin_tool_metadata_without_plugin_sources() {
    let tools = discover_tools(&ToolAssemblyInput {
        disable_plugin_tools: true,
        ..ToolAssemblyInput::default()
    })
    .await
    .expect("tool catalog");

    assert!(tools.windows(2).all(|pair| pair[0].name <= pair[1].name));
    let bash = tools
        .iter()
        .find(|tool| tool.name == "bash")
        .expect("bash tool");
    assert_eq!(bash.source, ToolCatalogSource::Builtin);
    assert!(!bash.description.is_empty());
    assert!(bash.effect.side_effects);
    assert!(tools
        .iter()
        .all(|tool| tool.source == ToolCatalogSource::Builtin));
}
