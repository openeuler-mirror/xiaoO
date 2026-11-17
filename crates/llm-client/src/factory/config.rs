use std::collections::BTreeMap;
use std::fmt;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::{resolve_provider_profile, LlmError, ProtocolFamily};

/// Optional TOML tables for literal request headers and environment references.
/// The presence of an empty table is retained for mutual exclusion checks.
#[derive(Clone, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct RequestHeadersConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_header: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_header_env: Option<BTreeMap<String, String>>,
}

impl fmt::Debug for RequestHeadersConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestHeadersConfig")
            .field("has_request_header", &self.request_header.is_some())
            .field("has_request_header_env", &self.request_header_env.is_some())
            .finish()
    }
}

impl RequestHeadersConfig {
    pub fn validate_for_provider(&self, provider: &str) -> Result<(), LlmError> {
        let protocol = resolve_provider_profile(provider).map(|profile| profile.protocol_family);
        self.resolve(protocol).map(|_| ())
    }

    pub fn is_empty(&self) -> bool {
        self.request_header.as_ref().is_none_or(BTreeMap::is_empty)
            && self
                .request_header_env
                .as_ref()
                .is_none_or(BTreeMap::is_empty)
    }

    pub(super) fn resolve(&self, protocol: Option<ProtocolFamily>) -> Result<HeaderMap, LlmError> {
        let (entries, from_env) = match (&self.request_header, &self.request_header_env) {
            (Some(_), Some(_)) => {
                return Err(LlmError::ConfigError(
                    "request_header and request_header_env are mutually exclusive".into(),
                ));
            }
            (Some(entries), None) => (entries, false),
            (None, Some(entries)) => (entries, true),
            (None, None) => return Ok(HeaderMap::new()),
        };

        if entries.is_empty() {
            return Ok(HeaderMap::new());
        }
        if protocol != Some(ProtocolFamily::OpenAiCompatible) {
            return Err(LlmError::ConfigError(
                "custom request headers require an OpenAI-compatible provider".into(),
            ));
        }

        let mut headers = HeaderMap::new();
        for (name, configured_value) in entries {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| LlmError::ConfigError("invalid custom request header name".into()))?;
            if matches!(
                name.as_str(),
                "authorization"
                    | "proxy-authorization"
                    | "content-type"
                    | "content-length"
                    | "host"
                    | "transfer-encoding"
                    | "connection"
                    | "keep-alive"
                    | "proxy-connection"
                    | "te"
                    | "trailer"
                    | "upgrade"
            ) {
                return Err(LlmError::ConfigError(format!(
                    "custom request header {name} is reserved"
                )));
            }
            if headers.contains_key(&name) {
                return Err(LlmError::ConfigError(format!(
                    "duplicate custom request header {name}"
                )));
            }

            let env_value;
            let value = if from_env {
                if !valid_env_name(configured_value) {
                    return Err(LlmError::ConfigError(format!(
                        "invalid environment variable name for custom request header {name}"
                    )));
                }
                env_value = std::env::var(configured_value).map_err(|error| {
                    let reason = match error {
                        std::env::VarError::NotPresent => "is not set",
                        std::env::VarError::NotUnicode(_) => "is not valid Unicode",
                    };
                    LlmError::ConfigError(format!(
                        "environment variable {configured_value} for custom request header {name} {reason}"
                    ))
                })?;
                &env_value
            } else {
                configured_value
            };

            if value.trim().is_empty() {
                return Err(LlmError::ConfigError(format!(
                    "custom request header {name} has an empty or whitespace-only value"
                )));
            }
            let mut value = HeaderValue::from_str(value).map_err(|_| {
                LlmError::ConfigError(format!("invalid value for custom request header {name}"))
            })?;
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        Ok(headers)
    }
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

#[derive(Debug, Clone)]
pub struct LlmProviderConfig {
    pub provider: String,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub model: String,
    pub request_headers: RequestHeadersConfig,
}

impl LlmProviderConfig {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            api_key: None,
            api_base: None,
            model: model.into(),
            request_headers: RequestHeadersConfig::default(),
        }
    }

    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = Some(api_base.into());
        self
    }

    pub fn with_request_headers(mut self, request_headers: RequestHeadersConfig) -> Self {
        self.request_headers = request_headers;
        self
    }
}
