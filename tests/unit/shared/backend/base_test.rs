use super::GatewayBackendConfig;

#[test]
fn backend_options_default_to_empty_object_when_omitted() {
    let config: GatewayBackendConfig =
        toml::from_str("kind = \"local\"").expect("backend config should parse");

    assert_eq!(config.options, serde_json::json!({}));
}
