use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::daemon_config::DaemonConfig;

const SDF_LIBRARY_PATH: &str = "/usr/local/sdf/lib/libsdf.so";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VaultSecretReference {
    pub path: String,
    pub environment: Option<String>,
    pub source: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct VaultReport {
    pub schema_version: u32,
    pub enabled: bool,
    pub use_sdf: bool,
    pub provider: String,
    pub store_path: PathBuf,
    pub store_exists: bool,
    pub store_size_bytes: Option<u64>,
    pub store_readable: bool,
    pub store_error_kind: Option<String>,
    pub sdf_library_path: PathBuf,
    pub sdf_available: bool,
    pub references: Vec<VaultSecretReference>,
}

pub fn vault_report(config: &DaemonConfig) -> VaultReport {
    let store_path = xiaoo_shared::llm_secrets::llm_secrets_path(&config.config_path);
    let metadata = std::fs::metadata(&store_path).ok();
    let store_exists = metadata.is_some();
    let store_size_bytes = metadata.as_ref().map(std::fs::Metadata::len);
    let (stored_names, store_readable, store_error_kind) = if store_exists {
        match xiaoo_shared::llm_secrets::inspect_secret_store(&config.config_path) {
            Ok(store) => (
                store
                    .api_keys
                    .into_iter()
                    .chain(store.tokens)
                    .collect::<BTreeSet<_>>(),
                true,
                None,
            ),
            Err(error) => (
                BTreeSet::new(),
                false,
                Some(classify_store_error(&error.to_string()).to_string()),
            ),
        }
    } else {
        (BTreeSet::new(), true, None)
    };

    let raw = std::fs::read_to_string(&config.config_path)
        .ok()
        .and_then(|content| content.parse::<toml::Value>().ok());
    let mut references = Vec::new();
    if let Some(raw) = raw.as_ref() {
        collect_references(raw, "", &stored_names, &mut references);
    }
    collect_default_model_references(config, &stored_names, &mut references);
    collect_effective_service_references(config, &stored_names, &mut references);
    references.sort_by(|left, right| left.path.cmp(&right.path));
    references.dedup_by(|left, right| left.path == right.path);

    let sdf_library_path = PathBuf::from(SDF_LIBRARY_PATH);
    VaultReport {
        schema_version: 1,
        enabled: config.app.vault.enabled,
        use_sdf: config.app.vault.use_sdf,
        provider: if config.app.vault.use_sdf {
            "sdf".to_string()
        } else {
            "whitebox".to_string()
        },
        store_path,
        store_exists,
        store_size_bytes,
        store_readable,
        store_error_kind,
        sdf_available: sdf_library_path.is_file(),
        sdf_library_path,
        references,
    }
}

fn collect_effective_service_references(
    config: &DaemonConfig,
    stored_names: &BTreeSet<String>,
    references: &mut Vec<VaultSecretReference>,
) {
    if config.app.mcp_server.enabled {
        for (path, configured, fallback) in [
            (
                "mcp_server.chatbot.bearer_token_env",
                config.app.mcp_server.chatbot.bearer_token_env.as_deref(),
                "XIAOO_MCP_CHATBOT_TOKEN",
            ),
            (
                "mcp_server.agent.bearer_token_env",
                config.app.mcp_server.agent.bearer_token_env.as_deref(),
                "XIAOO_MCP_AGENT_TOKEN",
            ),
        ] {
            let environment = configured
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(fallback);
            references.push(environment_reference(
                path.to_string(),
                environment,
                stored_names,
            ));
        }
    }
    if let Some(backend) = config.app.server.operation_backend.as_ref() {
        if backend.kind.trim() == "e2b" {
            let environment = backend
                .options
                .get("api_key_env")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("E2B_API_KEY");
            references.push(environment_reference(
                "server.operation_backend.options.api_key_env".to_string(),
                environment,
                stored_names,
            ));
        }
    }
}

fn collect_references(
    value: &toml::Value,
    parent: &str,
    stored_names: &BTreeSet<String>,
    references: &mut Vec<VaultSecretReference>,
) {
    let Some(table) = value.as_table() else {
        if let Some(array) = value.as_array() {
            for (index, item) in array.iter().enumerate() {
                collect_references(
                    item,
                    &format!("{parent}[{index}]"),
                    stored_names,
                    references,
                );
            }
        }
        return;
    };
    for (key, child) in table {
        let path = if parent.is_empty() {
            key.clone()
        } else {
            format!("{parent}.{key}")
        };
        if key.ends_with("_env") {
            if let Some(environment) = child
                .as_str()
                .map(str::trim)
                .filter(|name| !name.is_empty())
            {
                references.push(environment_reference(path, environment, stored_names));
            }
        } else if is_inline_secret(key)
            && child
                .as_str()
                .is_some_and(|secret| !secret.trim().is_empty())
        {
            references.push(VaultSecretReference {
                path,
                environment: None,
                source: "inline".to_string(),
                available: true,
            });
        } else {
            collect_references(child, &path, stored_names, references);
        }
    }
}

fn collect_default_model_references(
    config: &DaemonConfig,
    stored_names: &BTreeSet<String>,
    references: &mut Vec<VaultSecretReference>,
) {
    if config.app.llm.profiles.is_empty() {
        add_default_model_reference(
            "llm.api_key_env",
            &config.app.llm.provider,
            config.app.llm.api_key_env.as_deref(),
            stored_names,
            references,
        );
        return;
    }
    for (id, profile) in &config.app.llm.profiles {
        add_default_model_reference(
            &format!("llm.profiles.{id}.api_key_env"),
            &profile.provider,
            profile.api_key_env.as_deref(),
            stored_names,
            references,
        );
    }
}

fn add_default_model_reference(
    path: &str,
    provider: &str,
    configured: Option<&str>,
    stored_names: &BTreeSet<String>,
    references: &mut Vec<VaultSecretReference>,
) {
    let environment = configured
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            xiaoo_api::llm::resolve_provider_profile(provider.trim())
                .and_then(|profile| profile.default_api_key_env.map(str::to_string))
        });
    if let Some(environment) = environment {
        references.push(environment_reference(
            path.to_string(),
            &environment,
            stored_names,
        ));
    }
}

fn environment_reference(
    path: String,
    environment: &str,
    stored_names: &BTreeSet<String>,
) -> VaultSecretReference {
    let in_environment = std::env::var(environment)
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let in_store = stored_names.contains(environment);
    VaultSecretReference {
        path,
        environment: Some(environment.to_string()),
        source: if in_store {
            "encrypted_store"
        } else if in_environment {
            "environment"
        } else {
            "missing"
        }
        .to_string(),
        available: in_store || in_environment,
    }
}

fn is_inline_secret(key: &str) -> bool {
    matches!(
        key,
        "api_key" | "bearer_token" | "webhook_secret_token" | "verification_token"
    )
}

fn classify_store_error(error: &str) -> &'static str {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("sdf") || normalized.contains("libsdf") {
        "sdf_unavailable"
    } else if normalized.contains("decrypt") || normalized.contains("encryption version") {
        "decrypt_failed"
    } else if normalized.contains("parse") {
        "invalid_store"
    } else {
        "read_failed"
    }
}

pub fn vault_issues(config: &DaemonConfig) -> Vec<(String, String, String)> {
    if config.app.vault.use_sdf && !Path::new(SDF_LIBRARY_PATH).is_file() {
        return vec![(
            "vault.use_sdf".to_string(),
            "sdf_unavailable".to_string(),
            format!("SDF 已启用，但未找到 {SDF_LIBRARY_PATH}"),
        )];
    }
    Vec::new()
}

pub fn ensure_environment_is_referenced(
    config: &DaemonConfig,
    environment: &str,
) -> anyhow::Result<()> {
    let environment = environment.trim();
    if environment.is_empty()
        || !environment.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphanumeric()
                    && (index > 0 || character.is_ascii_alphabetic())
        })
    {
        anyhow::bail!("invalid secret environment variable name");
    }
    if vault_report(config)
        .references
        .iter()
        .any(|reference| reference.environment.as_deref() == Some(environment))
    {
        Ok(())
    } else {
        anyhow::bail!("secret environment variable is not referenced by the selected configuration")
    }
}

#[cfg(test)]
mod tests {
    use super::vault_report;
    use crate::daemon_config::{AppConfig, DaemonConfig};
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn reports_sources_without_exposing_secret_values() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        let secret = "vault-report-secret";
        std::fs::write(
            &path,
            format!(
                "[llm]\nprovider='openrouter'\nmodel='test'\napi_key_env='VAULT_REPORT_KEY'\n[http]\nbearer_token='{secret}'\n"
            ),
        )
        .expect("write config");
        xiaoo_shared::llm_secrets::save_llm_secret(&path, "VAULT_REPORT_KEY", secret)
            .expect("save secret");
        let report = vault_report(&DaemonConfig {
            app: toml::from_str::<AppConfig>(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            config_path: path,
        });
        assert!(report.store_readable);
        assert_eq!(report.references.len(), 2);
        assert!(report.references.iter().any(|reference| {
            reference.path == "llm.api_key_env" && reference.source == "encrypted_store"
        }));
        let json = serde_json::to_string(&report).expect("serialize report");
        assert!(!json.contains(secret));
    }

    #[test]
    fn reports_missing_sdf_library_as_unavailable() {
        let config = DaemonConfig {
            app: toml::from_str::<AppConfig>(
                "[llm]\nprovider='ollama'\nmodel='test'\n[vault]\nenabled=true\nuse_sdf=true\n",
            )
            .unwrap(),
            config_path: PathBuf::from("/tmp/config.toml"),
        };
        let report = vault_report(&config);
        assert_eq!(report.provider, "sdf");
        assert_eq!(report.sdf_available, report.sdf_library_path.is_file());
    }

    #[test]
    fn reports_effective_default_service_secret_references() {
        let config = DaemonConfig {
            app: toml::from_str::<AppConfig>(
                "[llm]\nprovider='ollama'\nmodel='test'\n[mcp_server]\nenabled=true\n[server.operation_backend]\nkind='e2b'\n",
            )
            .unwrap(),
            config_path: PathBuf::from("/tmp/config.toml"),
        };
        let report = vault_report(&config);
        for environment in [
            "XIAOO_MCP_CHATBOT_TOKEN",
            "XIAOO_MCP_AGENT_TOKEN",
            "E2B_API_KEY",
        ] {
            assert!(report
                .references
                .iter()
                .any(|reference| { reference.environment.as_deref() == Some(environment) }));
        }
    }
}
