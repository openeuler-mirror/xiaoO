#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolFamily {
    OpenAiCompatible,
    Anthropic,
    Gemini,
    Ollama,
    Zhipu,
}

impl ProtocolFamily {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openai" | "openai-compatible" => Some(Self::OpenAiCompatible),
            "anthropic" | "claude" => Some(Self::Anthropic),
            "gemini" | "google" => Some(Self::Gemini),
            "ollama" => Some(Self::Ollama),
            "zhipu" | "glm" => Some(Self::Zhipu),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Ollama => "ollama",
            Self::Zhipu => "zhipu",
        }
    }
}

impl std::fmt::Display for ProtocolFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiBaseStyle {
    Preserve,
    ExpectV1,
    ExpectNoV1,
}

pub fn normalize_api_base(api_base: &str, style: ApiBaseStyle) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');

    match style {
        ApiBaseStyle::Preserve => trimmed.to_string(),
        ApiBaseStyle::ExpectV1 => {
            if trimmed.ends_with("/v1") {
                trimmed.to_string()
            } else {
                format!("{trimmed}/v1")
            }
        }
        ApiBaseStyle::ExpectNoV1 => {
            if let Some(prefix) = trimmed.strip_suffix("/v1") {
                prefix.to_string()
            } else {
                trimmed.to_string()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderProfile {
    pub provider_name: &'static str,
    pub display_name: &'static str,
    pub protocol_family: ProtocolFamily,
    pub default_base_url: Option<&'static str>,
    pub default_api_key_env: Option<&'static str>,
    pub api_key_required: bool,
    pub api_base_style: ApiBaseStyle,
    pub supports_model_catalog: bool,
}

impl ProviderProfile {
    pub fn requires_api_key(&self) -> bool {
        self.api_key_required
    }
}

pub fn resolve_provider_profile(name: &str) -> Option<ProviderProfile> {
    let name = name.to_lowercase();

    match name.as_str() {
        "openai" => Some(ProviderProfile {
            provider_name: "openai",
            display_name: "OpenAI",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.openai.com/v1"),
            default_api_key_env: Some("OPENAI_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "anthropic" | "claude" => Some(ProviderProfile {
            provider_name: "anthropic",
            display_name: "Anthropic",
            protocol_family: ProtocolFamily::Anthropic,
            default_base_url: Some("https://api.anthropic.com/v1"),
            default_api_key_env: Some("ANTHROPIC_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "gemini" | "google" => Some(ProviderProfile {
            provider_name: "gemini",
            display_name: "Gemini",
            protocol_family: ProtocolFamily::Gemini,
            default_base_url: Some("https://generativelanguage.googleapis.com"),
            default_api_key_env: Some("GEMINI_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::Preserve,
            supports_model_catalog: true,
        }),
        "ollama" => Some(ProviderProfile {
            provider_name: "ollama",
            display_name: "Ollama",
            protocol_family: ProtocolFamily::Ollama,
            default_base_url: Some("http://localhost:11434"),
            default_api_key_env: None,
            api_key_required: false,
            api_base_style: ApiBaseStyle::Preserve,
            supports_model_catalog: true,
        }),
        "zai-cn" | "zai-china" | "zhipu" | "glm-cn" | "bigmodel" | "zai" | "z-ai" | "z.ai"
        | "zai-global" => Some(ProviderProfile {
            provider_name: "zhipu",
            display_name: "Zhipu",
            protocol_family: ProtocolFamily::Zhipu,
            default_base_url: Some("https://open.bigmodel.cn/api/paas/v4"),
            default_api_key_env: Some("ZHIPU_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::Preserve,
            supports_model_catalog: true,
        }),
        "deepseek" => Some(ProviderProfile {
            provider_name: "deepseek",
            display_name: "DeepSeek",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.deepseek.com"),
            default_api_key_env: Some("DEEPSEEK_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectNoV1,
            supports_model_catalog: true,
        }),
        "openrouter" => Some(ProviderProfile {
            provider_name: "openrouter",
            display_name: "OpenRouter",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://openrouter.ai/api/v1"),
            default_api_key_env: Some("OPENROUTER_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "openai-compatible" => Some(ProviderProfile {
            provider_name: "openai-compatible",
            display_name: "OpenAI-compatible",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: None,
            default_api_key_env: Some("OPENAI_COMPATIBLE_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::Preserve,
            supports_model_catalog: true,
        }),
        "groq" => Some(ProviderProfile {
            provider_name: "groq",
            display_name: "Groq",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.groq.com/openai/v1"),
            default_api_key_env: Some("GROQ_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "mistral" => Some(ProviderProfile {
            provider_name: "mistral",
            display_name: "Mistral",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.mistral.ai/v1"),
            default_api_key_env: Some("MISTRAL_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "together" => Some(ProviderProfile {
            provider_name: "together",
            display_name: "Together",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.together.xyz/v1"),
            default_api_key_env: Some("TOGETHER_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "xai" | "xai-grok" => Some(ProviderProfile {
            provider_name: "xai",
            display_name: "xAI",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.x.ai/v1"),
            default_api_key_env: Some("XAI_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "minimax" | "minimax-openai" => Some(ProviderProfile {
            provider_name: "minimax",
            display_name: "MiniMax",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.minimaxi.com/v1"),
            default_api_key_env: Some("MINIMAX_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: false,
        }),
        "minimax-anthropic" => Some(ProviderProfile {
            provider_name: "minimax-anthropic",
            display_name: "MiniMax (Anthropic)",
            protocol_family: ProtocolFamily::Anthropic,
            default_base_url: Some("https://api.minimaxi.com/anthropic/v1"),
            default_api_key_env: Some("MINIMAX_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::Preserve,
            supports_model_catalog: false,
        }),
        "gitcode" => Some(ProviderProfile {
            provider_name: "gitcode",
            display_name: "GitCode AI",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api-ai.gitcode.com/v1"),
            default_api_key_env: Some("GITCODE_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: false,
        }),
        // Z.AI Coding Plan (zhipu coding plan) — OpenAI-compatible endpoint at api.z.ai
        // Aliases: zai-coding-plan, zhipu-coding-plan, zhipuai-coding-plan
        "zai-coding-plan" | "zhipu-coding-plan" | "zhipuai-coding-plan" => Some(ProviderProfile {
            provider_name: "zai-coding-plan",
            display_name: "Z.AI Coding Plan",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.z.ai/api/coding/paas/v4"),
            default_api_key_env: Some("ZHIPU_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::Preserve,
            supports_model_catalog: true,
        }),
        "local" => Some(ProviderProfile {
            provider_name: "local",
            display_name: "Local Model",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("http://localhost:8080/v1"),
            default_api_key_env: None,
            api_key_required: false,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        "other" => Some(ProviderProfile {
            provider_name: "other",
            display_name: "Other",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://openrouter.ai/api/v1"),
            default_api_key_env: Some("OPENROUTER_API_KEY"),
            api_key_required: true,
            api_base_style: ApiBaseStyle::ExpectV1,
            supports_model_catalog: true,
        }),
        _ => None,
    }
}

pub fn resolve_protocol_family(name: &str) -> Option<ProtocolFamily> {
    if let Some(protocol) = ProtocolFamily::from_str(name) {
        return Some(protocol);
    }
    resolve_provider_profile(name).map(|p| p.protocol_family)
}

pub fn supported_providers() -> &'static [&'static str] {
    &[
        "openai",
        "other",
        "anthropic",
        "claude",
        "gemini",
        "google",
        "ollama",
        "zai",
        "zai-global",
        "z.ai",
        "zai-cn",
        "zai-china",
        "zhipu",
        "glm-cn",
        "bigmodel",
        "deepseek",
        "openrouter",
        "openai-compatible",
        "groq",
        "mistral",
        "together",
        "xai",
        "xai-grok",
        "minimax",
        "minimax-openai",
        "minimax-anthropic",
        "gitcode",
        "local",
        "zai-coding-plan",
        "zhipu-coding-plan",
        "zhipuai-coding-plan",
    ]
}

#[cfg(test)]
mod tests {
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

        assert!(resolve_provider_profile("unknown").is_none());
    }

    #[test]
    fn test_normalize_api_base() {
        assert_eq!(
            normalize_api_base("https://api.example.com", ApiBaseStyle::Preserve),
            "https://api.example.com"
        );
        assert_eq!(
            normalize_api_base("https://api.example.com/", ApiBaseStyle::ExpectV1),
            "https://api.example.com/v1"
        );
        assert_eq!(
            normalize_api_base("https://api.example.com/v1", ApiBaseStyle::ExpectV1),
            "https://api.example.com/v1"
        );
        assert_eq!(
            normalize_api_base("https://api.example.com/v1", ApiBaseStyle::ExpectNoV1),
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
}
