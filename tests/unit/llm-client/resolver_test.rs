use super::*;

#[test]
fn test_resolve_with_provider() {
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
fn test_resolve_preserves_openai_explicit_base_url() {
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
fn test_resolve_preserves_deepseek_explicit_v1_base_url() {
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
fn test_resolve_with_protocol_only() {
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
fn test_resolve_missing_protocol() {
    let input = ResolveInput {
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::MissingProtocol)));
}

#[test]
fn test_resolve_missing_base_url() {
    let input = ResolveInput {
        protocol: Some("openai".to_string()),
        api_key: Some("test-key".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::MissingBaseUrl)));
}

#[test]
fn test_resolve_missing_api_key() {
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
fn test_resolve_ollama_no_key_required() {
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
fn test_resolve_protocol_mismatch() {
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
fn test_resolve_unknown_provider() {
    let input = ResolveInput {
        provider: Some("unknown-provider".to_string()),
        ..Default::default()
    };
    let result = resolve_config(input);
    assert!(matches!(result, Err(ResolveError::ProviderNotFound(_))));
}
