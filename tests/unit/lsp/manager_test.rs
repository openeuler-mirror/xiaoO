use super::server_configs_with_disabled;
use crate::servers::{AutoInstall, ServerConfig};

#[test]
fn server_configs_filter_disabled_builtin_and_extra_servers() {
    let extra = ServerConfig {
        id: "custom-python",
        extensions: &["py"],
        command: "custom-python-lsp",
        args: &[],
        root_markers: &["pyproject.toml"],
        language_id: "python",
        initialization_options: None,
        auto_install: AutoInstall::None,
    };

    let configs = server_configs_with_disabled(
        vec![extra],
        &["pyright".to_string(), "custom-python".to_string()],
    );
    let ids = configs.iter().map(|config| config.id).collect::<Vec<_>>();

    assert!(!ids.contains(&"pyright"));
    assert!(!ids.contains(&"custom-python"));
    assert!(ids.contains(&"rust-analyzer"));
}
