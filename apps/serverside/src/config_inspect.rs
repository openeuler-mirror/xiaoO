use crate::config_schema::{config_schema, CONFIG_SCHEMA_VERSION};
use crate::config_validation::{validate_config_file, ConfigValidationIssue};
use crate::daemon_config::{DaemonConfig, LlmProfileConfig};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ConfigInspectOverrides {
    pub daemon_host: String,
    pub daemon_port: u16,
    pub no_dashboard: bool,
    pub dashboard_host: Option<String>,
    pub dashboard_port: Option<u16>,
    pub bearer_token_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct InspectedValue {
    pub value: Value,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedProfile {
    pub id: String,
    pub active: bool,
    pub enabled: bool,
    pub provider: String,
    pub model: String,
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key_status: String,
    pub context_window: Option<usize>,
    pub max_tokens: Option<usize>,
    pub reasoning_effort: String,
    pub kvcache_enabled: bool,
    pub kvcache_debug_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigInspectReport {
    pub schema_version: u32,
    pub valid: bool,
    pub config_path: PathBuf,
    pub active_profile: Option<String>,
    pub errors: Vec<ConfigValidationIssue>,
    pub values: BTreeMap<String, InspectedValue>,
    pub profiles: Vec<InspectedProfile>,
    pub config: Value,
}

pub fn inspect_config_file(
    config_path: &Path,
    overrides: &ConfigInspectOverrides,
) -> ConfigInspectReport {
    let validation = validate_config_file(config_path);
    let raw = fs::read_to_string(config_path)
        .ok()
        .and_then(|content| content.parse::<toml::Value>().ok());
    let mut config = raw
        .as_ref()
        .and_then(|value| serde_json::to_value(value).ok())
        .unwrap_or_else(|| json!({}));
    redact_value(&mut config, &[]);

    let mut values = BTreeMap::new();
    flatten_values(&config, "", &mut values);
    add_schema_defaults(&mut values);
    apply_startup_overrides(&mut values, overrides);

    let loaded = DaemonConfig::load_from(config_path).ok();
    let profiles = loaded
        .as_ref()
        .map(|config| inspect_profiles(config_path, config))
        .unwrap_or_default();

    ConfigInspectReport {
        schema_version: CONFIG_SCHEMA_VERSION,
        valid: validation.valid,
        config_path: config_path.to_path_buf(),
        active_profile: validation.active_profile,
        errors: validation.errors,
        values,
        profiles,
        config,
    }
}

fn inspect_profiles(config_path: &Path, config: &DaemonConfig) -> Vec<InspectedProfile> {
    if config.app.llm.profiles.is_empty() {
        return vec![profile_summary(
            config_path,
            "default",
            true,
            &LlmProfileConfig {
                enabled: true,
                provider: config.app.llm.provider.clone(),
                api_base: config.app.llm.api_base.clone(),
                api_key_env: config.app.llm.api_key_env.clone(),
                model: config.app.llm.model.clone(),
                context_window: config.app.llm.context_window,
                max_tokens: config.app.llm.max_tokens,
                reasoning_effort: Default::default(),
                kvcache_enabled: config.app.llm.kvcache_enabled,
                kvcache_debug_enabled: config.app.llm.kvcache_debug_enabled,
            },
        )];
    }
    config
        .app
        .llm
        .profiles
        .iter()
        .map(|(id, profile)| {
            profile_summary(
                config_path,
                id,
                config.app.llm.active_profile.as_deref() == Some(id.as_str()),
                profile,
            )
        })
        .collect()
}

fn profile_summary(
    config_path: &Path,
    id: &str,
    active: bool,
    profile: &LlmProfileConfig,
) -> InspectedProfile {
    let provider = xiaoo_api::llm::resolve_provider_profile(&profile.provider);
    let api_key_env = profile
        .api_key_env
        .clone()
        .or_else(|| provider.as_ref()?.default_api_key_env.map(str::to_string));
    let key_required = profile.api_key_env.is_some()
        || provider
            .as_ref()
            .is_some_and(xiaoo_api::llm::ProviderProfile::requires_api_key);
    let key_available = api_key_env
        .as_deref()
        .and_then(|env_name| xiaoo_shared::llm_secrets::get_llm_secret(config_path, env_name).ok())
        .is_some_and(|value| !value.trim().is_empty());
    let api_key_status = if key_available {
        "available"
    } else if key_required {
        "missing"
    } else {
        "not_required"
    };

    InspectedProfile {
        id: id.to_string(),
        active,
        enabled: profile.enabled,
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        api_base: profile
            .api_base
            .clone()
            .or_else(|| provider.and_then(|item| item.default_base_url.map(str::to_string))),
        api_key_env,
        api_key_status: api_key_status.to_string(),
        context_window: profile.context_window,
        max_tokens: profile.max_tokens,
        reasoning_effort: profile.reasoning_effort.to_string(),
        kvcache_enabled: profile.kvcache_enabled.unwrap_or(false),
        kvcache_debug_enabled: profile.kvcache_debug_enabled.unwrap_or(false),
    }
}

fn flatten_values(value: &Value, prefix: &str, values: &mut BTreeMap<String, InspectedValue>) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_values(child, &path, values);
            }
        }
        _ if !prefix.is_empty() => {
            values.insert(
                prefix.to_string(),
                InspectedValue {
                    value: value.clone(),
                    source: "config_file".to_string(),
                },
            );
        }
        _ => {}
    }
}

fn add_schema_defaults(values: &mut BTreeMap<String, InspectedValue>) {
    let schema = config_schema();
    let Some(sections) = schema.get("sections").and_then(Value::as_array) else {
        return;
    };
    for section in sections {
        let Some(path) = section.get("path").and_then(Value::as_str) else {
            continue;
        };
        if path.contains('*') || path.contains("[]") {
            continue;
        }
        let Some(fields) = section.get("fields").and_then(Value::as_array) else {
            continue;
        };
        for field in fields {
            let (Some(name), Some(default)) = (
                field.get("name").and_then(Value::as_str),
                field.get("default"),
            ) else {
                continue;
            };
            values
                .entry(format!("{path}.{name}"))
                .or_insert_with(|| InspectedValue {
                    value: default.clone(),
                    source: "default".to_string(),
                });
        }
    }
}

fn apply_startup_overrides(
    values: &mut BTreeMap<String, InspectedValue>,
    overrides: &ConfigInspectOverrides,
) {
    set_override(values, "daemon.host", json!(overrides.daemon_host));
    set_override(values, "daemon.port", json!(overrides.daemon_port));
    if overrides.no_dashboard {
        set_override(values, "http.dashboard.enabled", json!(false));
    }
    if let Some(host) = &overrides.dashboard_host {
        set_override(values, "http.dashboard.host", json!(host));
    }
    if let Some(port) = overrides.dashboard_port {
        set_override(values, "http.dashboard.port", json!(port));
    }
    if let Some(env_name) = &overrides.bearer_token_env {
        set_override(values, "http.bearer_token_env", json!(env_name));
    }
}

fn set_override(values: &mut BTreeMap<String, InspectedValue>, path: &str, value: Value) {
    values.insert(
        path.to_string(),
        InspectedValue {
            value,
            source: "command_line".to_string(),
        },
    );
}

fn redact_value(value: &mut Value, path: &[String]) {
    match value {
        Value::Object(object) => redact_object(object, path),
        Value::Array(items) => {
            for item in items {
                redact_value(item, path);
            }
        }
        _ => {}
    }
}

fn redact_object(object: &mut Map<String, Value>, path: &[String]) {
    for (key, value) in object {
        let mut child_path = path.to_vec();
        child_path.push(key.clone());
        let normalized = key.to_ascii_lowercase();
        let inside_headers = path
            .last()
            .is_some_and(|segment| segment.eq_ignore_ascii_case("headers"));
        let sensitive = inside_headers
            || (!normalized.ends_with("_env")
                && (normalized.contains("secret")
                    || normalized.contains("token")
                    || normalized == "api_key"
                    || normalized == "authorization"));
        if sensitive {
            *value = Value::String("<redacted>".to_string());
        } else {
            redact_value(value, &child_path);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/serverside/config_inspect_test.rs"]
mod tests;
