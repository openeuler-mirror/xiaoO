use super::server_configs_with_disabled;
use crate::servers::{AutoInstall, ServerConfig};

#[test]
fn server_configs_filter_disabled_builtin_and_extra_servers() {
    let extra = ServerConfig {
        id: "custom-python".to_string(),
        extensions: vec!["py".to_string()],
        command: "custom-python-lsp".to_string(),
        args: Vec::new(),
        root_markers: vec!["pyproject.toml".to_string()],
        language_id: "python".to_string(),
        initialization_options: None,
        auto_install: AutoInstall::None,
    };

    let configs = server_configs_with_disabled(
        vec![extra],
        &["pyright".to_string(), "custom-python".to_string()],
    );
    let ids = configs
        .iter()
        .map(|config| config.id.as_str())
        .collect::<Vec<_>>();

    assert!(!ids.contains(&"pyright"));
    assert!(!ids.contains(&"custom-python"));
    assert!(ids.contains(&"rust-analyzer"));
}
