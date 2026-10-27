use crate::providers::ReasoningDialect;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolFamily {
    OpenAiCompatible,
    Anthropic,
    Gemini,
    Ollama,
    Zhipu,
}

impl ProtocolFamily {
    pub fn parse(s: &str) -> Option<Self> {
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

    /// Static fallback context window for the protocol family: when the
    /// dynamic probing chain (provider catalog > OpenRouter > Ollama
    /// `/api/show`) misses entirely, the provider-construction default and
    /// the TUI's synchronous validation share this value so the two magic
    /// numbers cannot drift. A coarse lower bound only — not the real
    /// window (dynamic probing is authoritative).
    pub fn static_context_window(&self) -> usize {
        match self {
            Self::OpenAiCompatible | Self::Ollama | Self::Zhipu => 128_000,
            Self::Anthropic => 200_000,
            Self::Gemini => 1_000_000,
        }
    }
}

impl std::fmt::Display for ProtocolFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

pub fn normalize_api_base(api_base: &str) -> String {
    api_base.trim().trim_end_matches('/').to_string()
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderProfile {
    pub provider_name: &'static str,
    pub display_name: &'static str,
    pub protocol_family: ProtocolFamily,
    pub default_base_url: Option<&'static str>,
    pub default_api_key_env: Option<&'static str>,
    pub api_key_required: bool,
    pub supports_model_catalog: bool,
    /// Dialect for `reasoning_effort` values. Providers whose endpoint is
    /// the Zhipu API but speaks the OpenAI-compatible protocol (e.g.
    /// zai-coding-plan, api.z.ai/api/coding/paas/v4) must declare `Zhipu`;
    /// everyone else keeps the `OpenAi` default. Consumed by the factory's
    /// OpenAI-compatible branch; the Zhipu protocol family has it built in
    /// natively via `ZhipuProvider` and never reads this field.
    pub reasoning_dialect: ReasoningDialect,
}

impl ProviderProfile {
    pub fn requires_api_key(&self) -> bool {
        self.api_key_required
    }
}

/// Struct-update base for registry entries (`..BASE_PROFILE`):
/// `reasoning_dialect` defaults to OpenAi with only the Zhipu endpoints
/// (zhipu / zai-coding-plan) declaring `ReasoningDialect::Zhipu`
/// explicitly. Every other field value is a placeholder — each registry
/// entry must set its own, never relying on the defaults.
const BASE_PROFILE: ProviderProfile = ProviderProfile {
    provider_name: "",
    display_name: "",
    protocol_family: ProtocolFamily::OpenAiCompatible,
    default_base_url: None,
    default_api_key_env: None,
    api_key_required: true,
    supports_model_catalog: true,
    reasoning_dialect: ReasoningDialect::OpenAi,
};

impl Default for ProviderProfile {
    fn default() -> Self {
        BASE_PROFILE
    }
}

/// One provider's registration: `aliases` lists every recognized name
/// (lowercase, canonical name included and first as
/// `profile.provider_name`); `profile` is the assembly profile.
struct ProviderSpec {
    aliases: &'static [&'static str],
    profile: ProviderProfile,
}

/// The full provider registry: resolution (`resolve_provider_profile`),
/// the catalog (`provider_catalog`) and the supported list
/// (`supported_providers`) are all derived from this table.
/// **Adding a provider means adding one row** — there is no second list to
/// keep in sync. Table order is the catalog display order (common
/// providers first).
const PROVIDER_SPECS: &[ProviderSpec] = &[
    ProviderSpec {
        aliases: &["openai"],
        profile: ProviderProfile {
            provider_name: "openai",
            display_name: "OpenAI",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.openai.com/v1"),
            default_api_key_env: Some("OPENAI_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["other"],
        profile: ProviderProfile {
            provider_name: "other",
            display_name: "Other",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://openrouter.ai/api/v1"),
            default_api_key_env: Some("OPENROUTER_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["anthropic", "claude"],
        profile: ProviderProfile {
            provider_name: "anthropic",
            display_name: "Anthropic",
            protocol_family: ProtocolFamily::Anthropic,
            default_base_url: Some("https://api.anthropic.com/v1"),
            default_api_key_env: Some("ANTHROPIC_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["gemini", "google"],
        profile: ProviderProfile {
            provider_name: "gemini",
            display_name: "Gemini",
            protocol_family: ProtocolFamily::Gemini,
            default_base_url: Some("https://generativelanguage.googleapis.com"),
            default_api_key_env: Some("GEMINI_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["ollama"],
        profile: ProviderProfile {
            provider_name: "ollama",
            display_name: "Ollama",
            protocol_family: ProtocolFamily::Ollama,
            default_base_url: Some("http://localhost:11434"),
            default_api_key_env: None,
            api_key_required: false,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &[
            "zhipu",
            "zai-cn",
            "zai-china",
            "glm-cn",
            "bigmodel",
            "zai",
            "z-ai",
            "z.ai",
            "zai-global",
        ],
        profile: ProviderProfile {
            provider_name: "zhipu",
            display_name: "Zhipu",
            protocol_family: ProtocolFamily::Zhipu,
            default_base_url: Some("https://open.bigmodel.cn/api/paas/v4"),
            default_api_key_env: Some("ZHIPU_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            reasoning_dialect: ReasoningDialect::Zhipu,
        },
    },
    ProviderSpec {
        aliases: &["deepseek"],
        profile: ProviderProfile {
            provider_name: "deepseek",
            display_name: "DeepSeek",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.deepseek.com/v1"),
            default_api_key_env: Some("DEEPSEEK_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["openrouter"],
        profile: ProviderProfile {
            provider_name: "openrouter",
            display_name: "OpenRouter",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://openrouter.ai/api/v1"),
            default_api_key_env: Some("OPENROUTER_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["openai-compatible"],
        profile: ProviderProfile {
            provider_name: "openai-compatible",
            display_name: "OpenAI-compatible",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: None,
            default_api_key_env: Some("OPENAI_COMPATIBLE_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["groq"],
        profile: ProviderProfile {
            provider_name: "groq",
            display_name: "Groq",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.groq.com/openai/v1"),
            default_api_key_env: Some("GROQ_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["mistral"],
        profile: ProviderProfile {
            provider_name: "mistral",
            display_name: "Mistral",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.mistral.ai/v1"),
            default_api_key_env: Some("MISTRAL_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["together"],
        profile: ProviderProfile {
            provider_name: "together",
            display_name: "Together",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.together.xyz/v1"),
            default_api_key_env: Some("TOGETHER_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["xai", "xai-grok"],
        profile: ProviderProfile {
            provider_name: "xai",
            display_name: "xAI",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.x.ai/v1"),
            default_api_key_env: Some("XAI_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["minimax", "minimax-openai"],
        profile: ProviderProfile {
            provider_name: "minimax",
            display_name: "MiniMax",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.minimaxi.com/v1"),
            default_api_key_env: Some("MINIMAX_API_KEY"),
            api_key_required: true,
            supports_model_catalog: false,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["minimax-anthropic"],
        profile: ProviderProfile {
            provider_name: "minimax-anthropic",
            display_name: "MiniMax (Anthropic)",
            protocol_family: ProtocolFamily::Anthropic,
            default_base_url: Some("https://api.minimaxi.com/anthropic/v1"),
            default_api_key_env: Some("MINIMAX_API_KEY"),
            api_key_required: true,
            supports_model_catalog: false,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["kimi", "moonshot", "moonshot-ai"],
        profile: ProviderProfile {
            provider_name: "kimi",
            display_name: "Kimi",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.moonshot.cn/v1"),
            default_api_key_env: Some("MOONSHOT_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &[
            "minimax-coding-plan",
            "minimax-code-plan",
            "minimax-token-plan",
        ],
        profile: ProviderProfile {
            provider_name: "minimax-coding-plan",
            display_name: "MiniMax Coding Plan",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.minimax.io/v1"),
            default_api_key_env: Some("MINIMAX_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &[
            "kimi-coding-plan",
            "kimi-code-plan",
            "kimi-code",
            "kimi-for-coding",
        ],
        profile: ProviderProfile {
            provider_name: "kimi-coding-plan",
            display_name: "Kimi Coding Plan",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.kimi.com/coding/v1"),
            default_api_key_env: Some("KIMI_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["gitcode"],
        profile: ProviderProfile {
            provider_name: "gitcode",
            display_name: "GitCode AI",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api-ai.gitcode.com/v1"),
            default_api_key_env: Some("GITCODE_API_KEY"),
            api_key_required: true,
            supports_model_catalog: false,
            ..BASE_PROFILE
        },
    },
    ProviderSpec {
        aliases: &["local"],
        profile: ProviderProfile {
            provider_name: "local",
            display_name: "Local Model",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("http://localhost:8080/v1"),
            default_api_key_env: None,
            api_key_required: false,
            supports_model_catalog: true,
            ..BASE_PROFILE
        },
    },
    // Z.AI Coding Plan (zhipu coding plan) — OpenAI-compatible endpoint at api.z.ai
    ProviderSpec {
        aliases: &[
            "zai-coding-plan",
            "zhipu-coding-plan",
            "zhipuai-coding-plan",
        ],
        profile: ProviderProfile {
            provider_name: "zai-coding-plan",
            display_name: "Z.AI Coding Plan",
            protocol_family: ProtocolFamily::OpenAiCompatible,
            default_base_url: Some("https://api.z.ai/api/coding/paas/v4"),
            default_api_key_env: Some("ZHIPU_API_KEY"),
            api_key_required: true,
            supports_model_catalog: true,
            reasoning_dialect: ReasoningDialect::Zhipu,
        },
    },
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub name: String,
    pub display_name: String,
    pub protocol_family: String,
    pub default_base_url: Option<String>,
    pub default_api_key_env: Option<String>,
    pub api_key_required: bool,
    pub supports_model_catalog: bool,
    pub aliases: Vec<String>,
}

pub fn provider_catalog() -> Vec<ProviderMetadata> {
    PROVIDER_SPECS
        .iter()
        .map(|spec| {
            let profile = &spec.profile;
            ProviderMetadata {
                name: profile.provider_name.to_string(),
                display_name: profile.display_name.to_string(),
                protocol_family: profile.protocol_family.as_str().to_string(),
                default_base_url: profile.default_base_url.map(str::to_string),
                default_api_key_env: profile.default_api_key_env.map(str::to_string),
                api_key_required: profile.api_key_required,
                supports_model_catalog: profile.supports_model_catalog,
                aliases: spec
                    .aliases
                    .iter()
                    .filter(|alias| **alias != profile.provider_name)
                    .map(|alias| alias.to_string())
                    .collect(),
            }
        })
        .collect()
}

pub fn resolve_provider_profile(name: &str) -> Option<ProviderProfile> {
    let name = name.to_lowercase();
    PROVIDER_SPECS
        .iter()
        .find(|spec| spec.aliases.contains(&name.as_str()))
        .map(|spec| spec.profile)
}

pub fn resolve_protocol_family(name: &str) -> Option<ProtocolFamily> {
    if let Some(protocol) = ProtocolFamily::parse(name) {
        return Some(protocol);
    }
    resolve_provider_profile(name).map(|p| p.protocol_family)
}

static SUPPORTED_PROVIDERS: OnceLock<Vec<&'static str>> = OnceLock::new();

/// All recognized provider names (aliases included), derived from
/// [`PROVIDER_SPECS`].
pub fn supported_providers() -> &'static [&'static str] {
    SUPPORTED_PROVIDERS.get_or_init(|| {
        PROVIDER_SPECS
            .iter()
            .flat_map(|spec| spec.aliases.iter().copied())
            .collect()
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/provider_registry_test.rs"]
mod tests;
