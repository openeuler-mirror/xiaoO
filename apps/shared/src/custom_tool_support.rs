use std::path::Path;
pub use tool::{
    DeclarativeToolCatalog, DeclarativeToolDirectory, DeclarativeToolEffect, DeclarativeToolSummary,
};

pub fn custom_tool_catalog(
    workspace_root: Option<&Path>,
    home_dir: Option<&Path>,
    supported: bool,
) -> DeclarativeToolCatalog {
    tool::declarative_tool_catalog(workspace_root, home_dir, supported)
}
