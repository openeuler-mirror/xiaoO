use super::*;

#[test]
fn test_protocol_family_from_str() {
    assert_eq!(
        ProtocolFamily::from_str("openai"),
        Some(ProtocolFamily::OpenAiCompatible)
    );
    assert_eq!(
        ProtocolFamily::from_str("anthropic"),
        Some(ProtocolFamily::Anthropic)
    );
    assert_eq!(
        ProtocolFamily::from_str("gemini"),
        Some(ProtocolFamily::Gemini)
    );
    assert_eq!(
        ProtocolFamily::from_str("ollama"),
        Some(ProtocolFamily::Ollama)
    );
    assert_eq!(ProtocolFamily::from_str("unknown"), None);
}

#[test]
fn test_resolve_provider_profile() {
    let profile = resolve_provider_profile("openai").unwrap();
    assert_eq!(profile.provider_name, "openai");
    assert_eq!(profile.protocol_family, ProtocolFamily::OpenAiCompatible);
    assert!(profile.api_key_required);

    let profile = resolve_provider_profile("ollama").unwrap();
    assert_eq!(profile.provider_name, "ollama");
    assert!(!profile.api_key_required);

    let profile = resolve_provider_profile("kimi").unwrap();
    assert_eq!(profile.provider_name, "kimi");
    assert_eq!(profile.default_base_url, Some("https://api.moonshot.cn/v1"));
    assert_eq!(profile.default_api_key_env, Some("MOONSHOT_API_KEY"));
    assert_eq!(profile.protocol_family, ProtocolFamily::OpenAiCompatible);

    let profile = resolve_provider_profile("minimax-coding-plan").unwrap();
    assert_eq!(profile.provider_name, "minimax-coding-plan");
    assert_eq!(profile.default_base_url, Some("https://api.minimax.io/v1"));
    assert_eq!(profile.default_api_key_env, Some("MINIMAX_API_KEY"));
    assert_eq!(profile.protocol_family, ProtocolFamily::OpenAiCompatible);

    let profile = resolve_provider_profile("kimi-coding-plan").unwrap();
    assert_eq!(profile.provider_name, "kimi-coding-plan");
    assert_eq!(
        profile.default_base_url,
        Some("https://api.kimi.com/coding/v1")
    );
    assert_eq!(profile.default_api_key_env, Some("KIMI_API_KEY"));
    assert_eq!(profile.protocol_family, ProtocolFamily::OpenAiCompatible);

    assert!(resolve_provider_profile("unknown").is_none());
}

#[test]
fn provider_catalog_groups_aliases_by_canonical_name() {
    let catalog = provider_catalog();
    let anthropic = catalog
        .iter()
        .find(|provider| provider.name == "anthropic")
        .expect("anthropic metadata");
    assert!(anthropic.aliases.contains(&"claude".to_string()));
    assert_eq!(
        anthropic.default_api_key_env.as_deref(),
        Some("ANTHROPIC_API_KEY")
    );

    let zhipu = catalog
        .iter()
        .find(|provider| provider.name == "zhipu")
        .expect("zhipu metadata");
    assert!(zhipu.aliases.contains(&"zai".to_string()));
    assert!(catalog.iter().any(|provider| provider.name == "openrouter"));
    assert!(catalog.iter().any(|provider| provider.name == "local"));
}

#[test]
fn test_normalize_api_base() {
    assert_eq!(
        normalize_api_base("https://api.example.com"),
        "https://api.example.com"
    );
    assert_eq!(
        normalize_api_base("https://api.example.com/v1"),
        "https://api.example.com/v1"
    );
    assert_eq!(
        normalize_api_base("https://api.example.com/v1/"),
        "https://api.example.com/v1"
    );
    assert_eq!(
        normalize_api_base("  https://api.example.com/v1/  "),
        "https://api.example.com/v1"
    );
    assert_eq!(
        normalize_api_base("https://api.example.com/"),
        "https://api.example.com"
    );
}

#[test]
fn test_resolve_protocol_family() {
    assert_eq!(
        resolve_protocol_family("anthropic"),
        Some(ProtocolFamily::Anthropic)
    );
    assert_eq!(
        resolve_protocol_family("openrouter"),
        Some(ProtocolFamily::OpenAiCompatible)
    );
    assert_eq!(resolve_protocol_family("unknown"), None);
}
