use std::path::Path;
pub use tool::{
    DeclarativeToolCatalog, DeclarativeToolDirectory, DeclarativeToolDraft,
    DeclarativeToolDraftEffect, DeclarativeToolEffect, DeclarativeToolRenderReport,
    DeclarativeToolSummary, DeclarativeToolTestReport,
};

pub fn custom_tool_catalog(
    workspace_root: Option<&Path>,
    home_dir: Option<&Path>,
    supported: bool,
) -> DeclarativeToolCatalog {
    tool::declarative_tool_catalog(workspace_root, home_dir, supported)
}

pub fn render_custom_tool(draft: DeclarativeToolDraft) -> DeclarativeToolRenderReport {
    tool::render_declarative_tool(draft)
}

pub async fn test_custom_tool(
    workspace_root: &Path,
    home_dir: Option<&Path>,
    manifest_path: &Path,
    input: serde_json::Value,
    supported: bool,
    allow_effects: bool,
) -> DeclarativeToolTestReport {
    tool::test_declarative_tool(
        workspace_root,
        home_dir,
        manifest_path,
        input,
        supported,
        allow_effects,
    )
    .await
}
