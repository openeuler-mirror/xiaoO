use super::{install_lsp_server, lsp_catalog, test_lsp_server, ExtraServerConfigView};

struct Extra;

impl ExtraServerConfigView for Extra {
    fn id(&self) -> &str {
        "custom-lsp"
    }
    fn extensions(&self) -> &[String] {
        &[]
    }
    fn command(&self) -> &str {
        "missing-custom-lsp-command"
    }
    fn args(&self) -> &[String] {
        &[]
    }
    fn root_markers(&self) -> &[String] {
        &[]
    }
    fn language_id(&self) -> &str {
        "custom"
    }
}

#[test]
fn reports_builtin_and_custom_server_availability() {
    let report = lsp_catalog(true, &["rust-analyzer".to_string()], &[Extra]);
    assert_eq!(report.schema_version, 1);
    let rust = report
        .servers
        .iter()
        .find(|server| server.id == "rust-analyzer")
        .unwrap();
    assert!(!rust.configured);
    let custom = report
        .servers
        .iter()
        .find(|server| server.id == "custom-lsp")
        .unwrap();
    assert_eq!(custom.source, "custom");
    assert!(!custom.installed);
    assert!(!custom.startable);
}

#[tokio::test]
async fn actions_reject_disabled_and_unknown_servers_without_side_effects() {
    let disabled = install_lsp_server(false, &[], &[] as &[Extra], "gopls").await;
    assert!(!disabled.success);
    assert_eq!(disabled.error_kind, Some("disabled"));

    let unknown = test_lsp_server(
        true,
        &[],
        &[] as &[Extra],
        "missing-server",
        std::path::Path::new("."),
    )
    .await;
    assert!(!unknown.success);
    assert_eq!(unknown.error_kind, Some("unknown_server"));
}
