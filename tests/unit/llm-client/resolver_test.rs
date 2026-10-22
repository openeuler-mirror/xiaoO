use super::*;

#[test]
fn named_provider_resolves_protocol_default_base_url_and_key() {
    let input = ResolveInput {
        provider: Some("openai".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let config = resolve_config(input).unwrap();
    assert_eq!(config.provider, Some("openai".to_string()));
    assert_eq!(config.protocol, ProtocolFamily::OpenAiCompatible);
    assert_eq!(config.base_url, "https://api.openai.com/v1");
    assert_eq!(config.api_key, Some("test-key".to_string()));
    assert!(config.supports_model_catalog);
}

#[test]
fn resolve_preserves_openai_explicit_base_url() {
    let input = ResolveInput {
        provider: Some("openai".to_string()),
        base_url: Some("https://proxy.example.com".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let config = resolve_config(input).unwrap();
    assert_eq!(config.base_url, "https://proxy.example.com");
}

#[test]
fn resolve_preserves_deepseek_explicit_v1_base_url() {
    let input = ResolveInput {
        provider: Some("deepseek".to_string()),
        base_url: Some("https://api.deepseek.com/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let config = resolve_config(input).unwrap();
    assert_eq!(config.base_url, "https://api.deepseek.com/v1");
}

#[test]
fn protocol_without_provider_resolves_family_and_keeps_explicit_base_url() {
    let input = ResolveInput {
        protocol: Some("anthropic".to_string()),
        base_url: Some("https://custom.api.com/v1".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };

    let config = resolve_config(input).unwrap();
    assert_eq!(config.provider, None);
    assert_eq!(config.protocol, ProtocolFamily::Anthropic);
    assert_eq!(config.base_url, "https://custom.api.com/v1");
    assert!(!config.supports_model_catalog);
}

#[test]
fn missing_protocol_is_rejected() {
    let input = ResolveInput {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::MissingProtocol)));
}

#[test]
fn missing_base_url_is_rejected() {
    let input = ResolveInput {
        protocol: Some("openai".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::MissingBaseUrl)));
}

#[test]
fn missing_api_key_is_rejected() {
    temp_env::with_var_unset("OPENAI_API_KEY", || {
        let input = ResolveInput {
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let result = resolve_config(input);
        assert!(matches!(result, Err(ResolveError::MissingApiKey)));
    });
}

#[test]
fn ollama_resolves_without_api_key() {
    let input = ResolveInput {
        provider: Some("ollama".to_string()),
        ..Default::default()
    };
    let config = resolve_config(input).unwrap();
    assert_eq!(config.provider, Some("ollama".to_string()));
    assert_eq!(config.protocol, ProtocolFamily::Ollama);
    assert!(config.api_key.is_none());
}

#[test]
fn provider_protocol_mismatch_is_rejected() {
    let input = ResolveInput {
        provider: Some("openai".to_string()),
        protocol: Some("anthropic".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::ProtocolMismatch { .. })));
}

#[test]
fn unknown_provider_is_rejected() {
    let input = ResolveInput {
        provider: Some("unknown-provider".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::ProviderNotFound(_))));
}
