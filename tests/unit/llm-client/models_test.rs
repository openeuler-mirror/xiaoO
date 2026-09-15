use super::{
    extract_context_length_from_raw, find_model_summary, get_known_model_context_length,
    resolve_model_context_length, ModelSummary,
};
use crate::provider_registry::ProtocolFamily;
use crate::resolver::ResolvedConfig;

#[test]
fn find_model_summary_matches_gemini_prefixed_names() {
    let models = vec![ModelSummary::new("gemini-2.5-pro")];
    let found = find_model_summary(&models, "models/gemini-2.5-pro");
    assert!(found.is_some());
}

#[test]
fn extract_context_length_from_raw_finds_max_input_tokens() {
    let raw = serde_json::json!({
        "id": "claude-sonnet",
        "max_input_tokens": 200000
    });

    assert_eq!(extract_context_length_from_raw(&raw), Some(200000));
}

#[test]
fn known_model_context_matches_provider_prefixed_new_models() {
    assert_eq!(
        get_known_model_context_length("openai/gpt-5.6-luna"),
        Some(1_050_000)
    );
    assert_eq!(
        get_known_model_context_length("anthropic/claude-sonnet-5"),
        Some(1_000_000)
    );
    assert_eq!(
        get_known_model_context_length("moonshotai/kimi-k3"),
        Some(1_000_000)
    );
    assert_eq!(
        get_known_model_context_length("z-ai/glm-5.2"),
        Some(1_000_000)
    );
}

#[tokio::test]
async fn known_model_context_survives_catalog_failure() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/models")
        .with_status(503)
        .with_body("catalog unavailable")
        .create_async()
        .await;
    let config = ResolvedConfig {
        provider: Some("openai".to_string()),
        protocol: ProtocolFamily::OpenAiCompatible,
        api_key: Some("test-key".to_string()),
        base_url: server.url(),
        supports_model_catalog: true,
    };

    let resolved = resolve_model_context_length(&config, "gpt-5.6-sol")
        .await
        .unwrap();

    assert_eq!(resolved, Some(1_050_000));
    mock.assert_async().await;
}
