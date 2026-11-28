use std::collections::BTreeMap;
use std::fmt;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::de::{Error as DeError, MapAccess, Visitor};
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

type SelectedHeaderEntries<'a> = Option<(&'a BTreeMap<String, String>, bool)>;

impl RequestHeadersConfig {
    pub fn validate_for_provider(&self, provider: &str) -> Result<(), LlmError> {
        let protocol = resolve_provider_profile(provider).map(|profile| profile.protocol_family);
        self.resolve(protocol).map(|_| ())
    }

    /// Validate without reading env vars, so dormant entries fail before promotion.
    pub fn validate_structure_for_provider(&self, provider: &str) -> Result<(), LlmError> {
        let protocol =
            resolve_provider_profile(provider.trim()).map(|profile| profile.protocol_family);
        let Some((entries, from_env)) = self.selected_entries(protocol)? else {
            return Ok(());
        };
        let mut seen = HeaderMap::new();
        for (name, configured_value) in entries {
            let name = validate_header_entry(name, configured_value, from_env, &seen)?;
            seen.insert(name, HeaderValue::from_static(""));
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.request_header.as_ref().is_none_or(BTreeMap::is_empty)
            && self
                .request_header_env
                .as_ref()
                .is_none_or(BTreeMap::is_empty)
    }

    fn selected_entries(
        &self,
        protocol: Option<ProtocolFamily>,
    ) -> Result<SelectedHeaderEntries<'_>, LlmError> {
        let (entries, from_env) = match (&self.request_header, &self.request_header_env) {
            (Some(_), Some(_)) => {
                return Err(LlmError::ConfigError(
                    "request_header and request_header_env are mutually exclusive".into(),
                ));
            }
            (Some(entries), None) => (entries, false),
            (None, Some(entries)) => (entries, true),
            (None, None) => return Ok(None),
        };
        if entries.is_empty() {
            return Ok(None);
        }
        if protocol != Some(ProtocolFamily::OpenAiCompatible) {
            return Err(LlmError::ConfigError(
                "custom request headers require an OpenAI-compatible provider".into(),
            ));
        }
        Ok(Some((entries, from_env)))
    }

    pub(super) fn resolve(&self, protocol: Option<ProtocolFamily>) -> Result<HeaderMap, LlmError> {
        let Some((entries, from_env)) = self.selected_entries(protocol)? else {
            return Ok(HeaderMap::new());
        };

        let mut headers = HeaderMap::new();
        for (name, configured_value) in entries {
            let name = validate_header_entry(name, configured_value, from_env, &headers)?;
            let mut value = if from_env {
                let env_value = std::env::var(configured_value).map_err(|error| {
                    let reason = match error {
                        std::env::VarError::NotPresent => "is not set",
                        std::env::VarError::NotUnicode(_) => "is not valid Unicode",
                    };
                    LlmError::ConfigError(format!(
                        "environment variable {configured_value} for custom request header {name} {reason}"
                    ))
                })?;
                header_value(&name, &env_value)?
            } else {
                header_value(&name, configured_value)?
            };
            value.set_sensitive(true);
            headers.insert(name, value);
        }
        Ok(headers)
    }
}

fn validate_header_entry(
    name: &str,
    configured_value: &str,
    from_env: bool,
    seen: &HeaderMap,
) -> Result<HeaderName, LlmError> {
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
    if seen.contains_key(&name) {
        return Err(LlmError::ConfigError(format!(
            "duplicate custom request header {name}"
        )));
    }
    if from_env {
        if !valid_env_name(configured_value) {
            return Err(LlmError::ConfigError(format!(
                "invalid environment variable name for custom request header {name}"
            )));
        }
    } else {
        header_value(&name, configured_value)?;
    }
    Ok(name)
}

fn header_value(name: &HeaderName, value: &str) -> Result<HeaderValue, LlmError> {
    if value.trim().is_empty() {
        return Err(LlmError::ConfigError(format!(
            "custom request header {name} has an empty or whitespace-only value"
        )));
    }
    HeaderValue::from_str(value).map_err(|_| {
        LlmError::ConfigError(format!("invalid value for custom request header {name}"))
    })
}

fn trimmed_non_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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

/// One `[[llm.models]]` entry; hand-written Deserialize because
/// `#[serde(flatten)]` is incompatible with `deny_unknown_fields`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct LlmModelEntry {
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(flatten)]
    pub request_headers: RequestHeadersConfig,
}

const MODEL_ENTRY_FIELDS: &[&str] = &[
    "provider",
    "model",
    "api_base",
    "api_key_env",
    "request_header",
    "request_header_env",
];

const REQUIRED_ENTRY_FIELDS: &[&str] = &["provider", "model"];

fn take_entry_field<'de, A, T>(
    slot: &mut Option<T>,
    name: &'static str,
    map: &mut A,
) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(DeError::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

impl<'de> Deserialize<'de> for LlmModelEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ModelEntryVisitor;

        impl<'de> Visitor<'de> for ModelEntryVisitor {
            type Value = LlmModelEntry;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an [[llm.models]] entry table")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut provider = None;
                let mut model = None;
                let mut api_base = None;
                let mut api_key_env = None;
                let mut request_header = None;
                let mut request_header_env = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "provider" => take_entry_field(&mut provider, "provider", &mut map)?,
                        "model" => take_entry_field(&mut model, "model", &mut map)?,
                        "api_base" => take_entry_field(&mut api_base, "api_base", &mut map)?,
                        "api_key_env" => {
                            take_entry_field(&mut api_key_env, "api_key_env", &mut map)?
                        }
                        "request_header" => {
                            take_entry_field(&mut request_header, "request_header", &mut map)?
                        }
                        "request_header_env" => take_entry_field(
                            &mut request_header_env,
                            "request_header_env",
                            &mut map,
                        )?,
                        unknown => {
                            return Err(DeError::unknown_field(unknown, MODEL_ENTRY_FIELDS));
                        }
                    }
                }

                let provider = provider.ok_or_else(|| DeError::missing_field("provider"))?;
                let model = model.ok_or_else(|| DeError::missing_field("model"))?;

                Ok(LlmModelEntry {
                    provider,
                    model,
                    api_base,
                    api_key_env,
                    request_headers: RequestHeadersConfig {
                        request_header,
                        request_header_env,
                    },
                })
            }
        }

        deserializer.deserialize_map(ModelEntryVisitor)
    }
}

/// Flat `[llm]` connection snapshot; blank values count as unset.
#[derive(Debug, Clone, Default)]
pub struct FlatLlmConnection {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key_env: Option<String>,
    pub api_base: Option<String>,
    pub request_headers: RequestHeadersConfig,
    pub profiles_present: bool,
    pub active_profile_present: bool,
}

impl FlatLlmConnection {
    pub fn from_parts(
        provider: Option<String>,
        model: Option<String>,
        api_key_env: Option<String>,
        api_base: Option<String>,
        request_headers: RequestHeadersConfig,
        profile_entries: usize,
        active_profile: Option<&str>,
    ) -> Self {
        let (profiles_present, active_profile_present) =
            Self::profile_key_flags(profile_entries, active_profile);
        Self {
            provider,
            model,
            api_key_env,
            api_base,
            request_headers,
            profiles_present,
            active_profile_present,
        }
    }

    /// An empty `[llm.profiles]` table and a blank `active_profile`
    /// read as unset.
    pub fn profile_key_flags(profile_entries: usize, active_profile: Option<&str>) -> (bool, bool) {
        (
            profile_entries > 0,
            active_profile.is_some_and(|value| !value.trim().is_empty()),
        )
    }

    pub fn conflicting_keys_with_models(&self) -> Vec<&'static str> {
        let non_blank =
            |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
        let conflicts = [
            ("llm.provider", non_blank(&self.provider)),
            ("llm.model", non_blank(&self.model)),
            ("llm.api_key_env", non_blank(&self.api_key_env)),
            ("llm.api_base", non_blank(&self.api_base)),
            (
                "llm.request_header",
                self.request_headers.request_header.is_some(),
            ),
            (
                "llm.request_header_env",
                self.request_headers.request_header_env.is_some(),
            ),
        ];
        conflicts
            .into_iter()
            .filter(|(_, conflicts)| *conflicts)
            .map(|(key, _)| key)
            .collect()
    }
}

impl LlmModelEntry {
    pub fn entry_schema_hint() -> String {
        let required = REQUIRED_ENTRY_FIELDS
            .iter()
            .map(|field| format!("`{field}`"))
            .collect::<Vec<_>>()
            .join(" and ");
        let optional = MODEL_ENTRY_FIELDS
            .iter()
            .filter(|field| !REQUIRED_ENTRY_FIELDS.contains(field))
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        format!("an entry must declare non-blank {required} and may only use the known entry keys ({optional})")
    }

    /// Shared activation rule: reject coexisting flat/profile keys,
    /// validate every entry, return entry 0.
    pub fn activate_entry(
        models: &[LlmModelEntry],
        flat: &FlatLlmConnection,
    ) -> Result<LlmModelEntry, LlmError> {
        if flat.profiles_present {
            return Err(LlmError::ConfigError(
                "[[llm.models]] is mutually exclusive with [llm.profiles] (remove one of them)"
                    .into(),
            ));
        }
        if flat.active_profile_present {
            return Err(LlmError::ConfigError(
                "[[llm.models]] is mutually exclusive with llm.active_profile (remove one of them)"
                    .into(),
            ));
        }
        let conflicting = flat.conflicting_keys_with_models();
        if !conflicting.is_empty() {
            return Err(LlmError::ConfigError(format!(
                "[[llm.models]] is mutually exclusive with {} (remove the flat keys or the models array)",
                conflicting.join(", ")
            )));
        }
        let active = models.first().ok_or_else(|| {
            LlmError::ConfigError(
                "[[llm.models]] must contain at least one entry — add an entry or remove the `models` key"
                    .into(),
            )
        })?;
        Self::validate_entries(models)?;
        Ok(active.clone())
    }

    /// Empty arrays pass; dormant entries are validated too.
    pub fn validate_entries(models: &[LlmModelEntry]) -> Result<(), LlmError> {
        for (index, entry) in models.iter().enumerate() {
            if entry.provider.trim().is_empty() {
                return Err(LlmError::ConfigError(format!(
                    "llm.models[{index}].provider is required"
                )));
            }
            if entry.model.trim().is_empty() {
                return Err(LlmError::ConfigError(format!(
                    "llm.models[{index}].model is required"
                )));
            }
            if resolve_provider_profile(entry.provider.trim()).is_none() {
                return Err(LlmError::ConfigError(format!(
                    "llm.models[{index}].provider '{}' is not a known provider",
                    entry.provider.trim()
                )));
            }
            entry
                .request_headers
                .validate_structure_for_provider(&entry.provider)
                .map_err(|error| LlmError::ConfigError(format!("llm.models[{index}]: {error}")))?;
        }
        Ok(())
    }

    pub fn trimmed(&self) -> LlmModelEntry {
        LlmModelEntry {
            provider: self.provider.trim().to_string(),
            model: self.model.trim().to_string(),
            api_base: trimmed_non_blank(self.api_base.as_deref()),
            api_key_env: trimmed_non_blank(self.api_key_env.as_deref()),
            request_headers: self.request_headers.clone(),
        }
    }

    pub fn copy_into_flat(&self, target: &mut impl FlatLlmConnectionTarget) {
        let trimmed = self.trimmed();
        target.set_provider(trimmed.provider);
        target.set_model(trimmed.model);
        target.set_api_base(trimmed.api_base);
        target.set_api_key_env(trimmed.api_key_env);
        target.set_request_headers(trimmed.request_headers);
    }
}

pub trait FlatLlmConnectionTarget {
    fn set_provider(&mut self, provider: String);
    fn set_model(&mut self, model: String);
    fn set_api_base(&mut self, api_base: Option<String>);
    fn set_api_key_env(&mut self, api_key_env: Option<String>);
    fn set_request_headers(&mut self, request_headers: RequestHeadersConfig);
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
