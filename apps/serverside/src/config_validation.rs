use crate::compact_management::compact_issues;
use crate::daemon_config::{DaemonConfig, LlmProfileConfig};
use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfigValidationIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigValidationReport {
    pub valid: bool,
    pub config_path: PathBuf,
    pub active_profile: Option<String>,
    pub errors: Vec<ConfigValidationIssue>,
}

impl ConfigValidationReport {
    fn from_error(config_path: &Path, error: anyhow::Error) -> Self {
        Self {
            valid: false,
            config_path: config_path.to_path_buf(),
            active_profile: None,
            errors: vec![ConfigValidationIssue {
                path: "config".to_string(),
                code: "config_load_failed".to_string(),
                message: format!("{error:#}"),
            }],
        }
    }
}

pub fn validate_config_file(config_path: &Path) -> ConfigValidationReport {
    let config = match DaemonConfig::load_from(config_path) {
        Ok(config) => config,
        Err(error) => return ConfigValidationReport::from_error(config_path, error),
    };
    let mut errors = Vec::new();
    if config.app.llm.profiles.is_empty() {
        validate_profile(
            "llm",
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
            config_path,
            &mut errors,
        );
    } else {
        for (profile_id, profile) in &config.app.llm.profiles {
            if profile.enabled {
                validate_profile(
                    &format!("llm.profiles.{profile_id}"),
                    profile,
                    config_path,
                    &mut errors,
                );
            }
        }
    }
    if let Some(compact) = config.app.compact.as_ref() {
        errors.extend(
            compact_issues(compact)
                .into_iter()
                .map(|issue| ConfigValidationIssue {
                    path: issue.path,
                    code: issue.code,
                    message: issue.message,
                }),
        );
    }

    ConfigValidationReport {
        valid: errors.is_empty(),
        config_path: config_path.to_path_buf(),
        active_profile: config.app.llm.active_profile,
        errors,
    }
}

fn validate_profile(
    path: &str,
    profile: &LlmProfileConfig,
    config_path: &Path,
    errors: &mut Vec<ConfigValidationIssue>,
) {
    if profile.provider.trim().is_empty() {
        push_error(
            errors,
            format!("{path}.provider"),
            "required",
            "LLM Provider 不能为空",
        );
        return;
    }
    if profile.model.trim().is_empty() {
        push_error(
            errors,
            format!("{path}.model"),
            "required",
            "模型名称不能为空",
        );
    }
    if matches!(profile.context_window, Some(0)) {
        push_error(
            errors,
            format!("{path}.context_window"),
            "out_of_range",
            "context_window 必须大于 0",
        );
    }
    if matches!(profile.max_tokens, Some(0)) {
        push_error(
            errors,
            format!("{path}.max_tokens"),
            "out_of_range",
            "max_tokens 必须大于 0",
        );
    }

    let Some(provider) = xiaoo_api::llm::resolve_provider_profile(profile.provider.trim()) else {
        push_error(
            errors,
            format!("{path}.provider"),
            "unknown_provider",
            format!("不支持的 LLM Provider：{}", profile.provider.trim()),
        );
        return;
    };

    let api_base = profile
        .api_base
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or(provider.default_base_url);
    match api_base {
        None => push_error(
            errors,
            format!("{path}.api_base"),
            "missing_api_base",
            format!("{} 需要配置 API Base", provider.display_name),
        ),
        Some(value) => match Url::parse(value) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {}
            _ => push_error(
                errors,
                format!("{path}.api_base"),
                "invalid_url",
                "API Base 必须是合法的 HTTP(S) URL",
            ),
        },
    }

    let explicit_env = profile
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(env_name) = explicit_env {
        let valid_name =
            Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("API key environment variable regex");
        if !valid_name.is_match(env_name) {
            push_error(
                errors,
                format!("{path}.api_key_env"),
                "invalid_environment_variable",
                "API Key 环境变量名不合法",
            );
            return;
        }
    }
    let required_env = explicit_env.or(provider.default_api_key_env);
    let key_required = explicit_env.is_some() || provider.requires_api_key();
    if key_required {
        let available = required_env
            .and_then(|env_name| {
                xiaoo_shared::llm_secrets::get_llm_secret(config_path, env_name).ok()
            })
            .is_some_and(|value| !value.trim().is_empty());
        if !available {
            let message = required_env
                .map(|env_name| format!("缺少 API Key：请设置环境变量或密钥引用 {env_name}"))
                .unwrap_or_else(|| "当前 Provider 需要 API Key".to_string());
            push_error(
                errors,
                format!("{path}.api_key_env"),
                "missing_api_key",
                message,
            );
        }
    }
}

fn push_error(
    errors: &mut Vec<ConfigValidationIssue>,
    path: String,
    code: &str,
    message: impl Into<String>,
) {
    errors.push(ConfigValidationIssue {
        path,
        code: code.to_string(),
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::validate_config_file;
    use tempfile::TempDir;

    #[test]
    fn rejects_missing_base_and_api_key_before_daemon_start() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[llm]
active_profile = "broken"

[llm.profiles.broken]
provider = "openai-compatible"
model = "test-model"
api_key_env = "XIAOO_VALIDATION_MISSING_KEY_42"
"#,
        )
        .expect("write config");

        let report = validate_config_file(&path);
        assert!(!report.valid);
        assert!(report.errors.iter().any(|issue| {
            issue.path == "llm.profiles.broken.api_base" && issue.code == "missing_api_base"
        }));
        assert!(report.errors.iter().any(|issue| {
            issue.path == "llm.profiles.broken.api_key_env" && issue.code == "missing_api_key"
        }));
    }

    #[test]
    fn validates_every_enabled_profile() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[llm]
active_profile = "primary"

[llm.profiles.primary]
provider = "ollama"
model = "qwen"

[llm.profiles.secondary]
provider = "not-a-provider"
model = "other"

[llm.profiles.ignored]
enabled = false
provider = "also-unknown"
model = "ignored"
"#,
        )
        .expect("write config");

        let report = validate_config_file(&path);
        assert!(!report.valid);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].path, "llm.profiles.secondary.provider");
        assert_eq!(report.errors[0].code, "unknown_provider");
    }

    #[test]
    fn accepts_valid_local_profile_without_api_key() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
[llm]
active_profile = "local"

[llm.profiles.local]
provider = "ollama"
model = "qwen"
"#,
        )
        .expect("write config");

        let report = validate_config_file(&path);
        assert!(report.valid, "{:?}", report.errors);
    }

    #[test]
    fn accepts_api_key_from_the_selected_config_secret_store() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        let env_name = "XIAOO_VALIDATION_ENCRYPTED_KEY_42";
        std::env::remove_var(env_name);
        std::fs::write(
            &path,
            format!(
                r#"
[llm]
active_profile = "remote"

[llm.profiles.remote]
provider = "openrouter"
model = "test-model"
api_key_env = "{env_name}"
"#,
            ),
        )
        .expect("write config");
        xiaoo_shared::llm_secrets::save_llm_secret(&path, env_name, "secret-key")
            .expect("save secret");

        let report = validate_config_file(&path);
        assert!(report.valid, "{:?}", report.errors);
    }

    #[test]
    fn rejects_invalid_compact_configuration_before_daemon_start() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[compact]\nwarning_ratio = 0.95\nauto_compact_ratio = 0.75\nblocking_ratio = 0.9\nsummary_llm_max_tokens = 0\n",
        )
        .expect("write config");

        let report = validate_config_file(&path);

        assert!(!report.valid);
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.code == "invalid_threshold_order"));
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.path == "compact.summary_llm_max_tokens"));
    }
}
