use std::env;

use crate::error::LlmError;
use crate::provider_registry::{
    normalize_api_base, resolve_provider_profile, ApiBaseStyle, ProtocolFamily, ProviderProfile,
};

#[derive(Debug, Clone, Default)]
pub struct ResolveInput {
    pub provider: Option<String>,
    pub protocol: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub provider: Option<String>,
    pub protocol: ProtocolFamily,
    pub api_key: Option<String>,
    pub base_url: String,
    pub supports_model_catalog: bool,
}

#[derive(Debug, Clone)]
pub enum ResolveError {
    MissingProtocol,
    MissingBaseUrl,
    MissingApiKey,
    ProviderNotFound(String),
    ProtocolMismatch { provider: String, requested: String },
    EnvVarNotFound(String),
    EnvVarNotUnicode(String),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingProtocol => {
                write!(f, "missing protocol: specify --protocol or --provider")
            }
            Self::MissingBaseUrl => write!(
                f,
                "missing base_url: specify --base_url or --provider with default"
            ),
            Self::MissingApiKey => write!(
                f,
                "missing API key: specify --api_key, --api_key_env, or --provider with default"
            ),
            Self::ProviderNotFound(name) => write!(
                f,
                "unknown provider: {}. Supported: {}",
                name,
                crate::provider_registry::supported_providers().join(", ")
            ),
            Self::ProtocolMismatch {
                provider,
                requested,
            } => write!(
                f,
                "protocol mismatch: provider '{}' uses different protocol than '{}'",
                provider, requested
            ),
            Self::EnvVarNotFound(name) => write!(f, "environment variable not found: {}", name),
            Self::EnvVarNotUnicode(name) => {
                write!(f, "environment variable is not valid unicode: {}", name)
            }
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<ResolveError> for LlmError {
    fn from(err: ResolveError) -> Self {
        LlmError::ConfigError(err.to_string())
    }
}

pub fn resolve_config(input: ResolveInput) -> Result<ResolvedConfig, ResolveError> {
    let profile: Option<ProviderProfile> = match input.provider.as_deref() {
        Some(p) => Some(
            resolve_provider_profile(p)
                .ok_or_else(|| ResolveError::ProviderNotFound(p.to_string()))?,
        ),
        None => None,
    };

    let protocol = resolve_protocol(&input.protocol, profile.as_ref())?;

    if let (Some(ref profile), Some(ref explicit_protocol)) = (&profile, &input.protocol) {
        let explicit_family = ProtocolFamily::from_str(explicit_protocol);
        if let Some(explicit) = explicit_family {
            if explicit != profile.protocol_family {
                return Err(ResolveError::ProtocolMismatch {
                    provider: input.provider.clone().unwrap(),
                    requested: explicit_protocol.clone(),
                });
            }
        }
    }

    let api_base_style = profile
        .as_ref()
        .map(|p| p.api_base_style)
        .unwrap_or(ApiBaseStyle::Preserve);
    let base_url = resolve_base_url(&input.base_url, profile.as_ref(), api_base_style)?;

    let api_key_required = profile.as_ref().map(|p| p.api_key_required).unwrap_or(true);
    let api_key = resolve_api_key(
        &input.api_key,
        &input.api_key_env,
        profile.as_ref(),
        api_key_required,
    )?;

    let supports_model_catalog = profile
        .as_ref()
        .map(|p| p.supports_model_catalog)
        .unwrap_or(false);

    Ok(ResolvedConfig {
        provider: input.provider,
        protocol,
        api_key,
        base_url,
        supports_model_catalog,
    })
}

fn resolve_protocol(
    explicit: &Option<String>,
    profile: Option<&ProviderProfile>,
) -> Result<ProtocolFamily, ResolveError> {
    if let Some(ref proto) = explicit {
        return ProtocolFamily::from_str(proto).ok_or(ResolveError::MissingProtocol);
    }
    if let Some(p) = profile {
        return Ok(p.protocol_family);
    }
    Err(ResolveError::MissingProtocol)
}

fn resolve_base_url(
    explicit: &Option<String>,
    profile: Option<&ProviderProfile>,
    style: ApiBaseStyle,
) -> Result<String, ResolveError> {
    if let Some(ref url) = explicit {
        let url = url.trim();
        if url.is_empty() {
            return Err(ResolveError::MissingBaseUrl);
        }
        return Ok(normalize_api_base(url, style));
    }
    if let Some(p) = profile {
        if let Some(url) = p.default_base_url {
            return Ok(normalize_api_base(url, style));
        }
    }
    Err(ResolveError::MissingBaseUrl)
}

fn resolve_api_key(
    explicit: &Option<String>,
    env_name: &Option<String>,
    profile: Option<&ProviderProfile>,
    required: bool,
) -> Result<Option<String>, ResolveError> {
    if let Some(ref key) = explicit {
        let key = key.trim();
        if !key.is_empty() {
            return Ok(Some(key.to_string()));
        }
    }

    if let Some(ref name) = env_name {
        return match env::var(name) {
            Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
            Ok(_) => Err(ResolveError::EnvVarNotFound(name.clone())),
            Err(env::VarError::NotPresent) => Err(ResolveError::EnvVarNotFound(name.clone())),
            Err(env::VarError::NotUnicode(_)) => Err(ResolveError::EnvVarNotUnicode(name.clone())),
        };
    }

    if let Some(p) = profile {
        if let Some(name) = p.default_api_key_env {
            match env::var(name) {
                Ok(value) if !value.trim().is_empty() => return Ok(Some(value)),
                Ok(_) | Err(env::VarError::NotPresent) => {}
                Err(env::VarError::NotUnicode(_)) => {
                    return Err(ResolveError::EnvVarNotUnicode(name.to_string()));
                }
            }
        }
    }

    if !required {
        return Ok(None);
    }

    Err(ResolveError::MissingApiKey)
}

#[cfg(test)]
mod tests {
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
        let input = ResolveInput {
            provider: Some("openai".to_string()),
            ..Default::default()
        };
        let result = resolve_config(input);
        assert!(matches!(result, Err(ResolveError::MissingApiKey)));
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
}
