use crate::backend_management::backend_issues;
use crate::channel_management::channel_report;
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
    if let Some(backend) = config.app.server.operation_backend.as_ref() {
        errors.extend(
            backend_issues(backend.kind.trim(), &backend.options)
                .into_iter()
                .map(|issue| ConfigValidationIssue {
                    path: issue.path,
                    code: issue.code,
                    message: issue.message,
                }),
        );
    }
    errors.extend(
        channel_report(&config)
            .channels
            .into_iter()
            .flat_map(|channel| channel.errors)
            .map(|issue| ConfigValidationIssue {
                path: issue.path,
                code: issue.code,
                message: issue.message,
            }),
    );
    validate_http(&config, &mut errors);

    ConfigValidationReport {
        valid: errors.is_empty(),
        config_path: config_path.to_path_buf(),
        active_profile: config.app.llm.active_profile,
        errors,
    }
}

fn validate_http(config: &DaemonConfig, errors: &mut Vec<ConfigValidationIssue>) {
    let http = &config.app.http;
    let inline = http
        .bearer_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    let environment = http
        .bearer_token_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if inline && environment.is_some() {
        push_error(
            errors,
            "http.bearer_token".to_string(),
            "conflicting_secret_sources",
            "bearer_token 与 bearer_token_env 不能同时配置",
        );
    }
    if let Some(name) = environment {
        let valid_name =
            Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("environment variable regex");
        if !valid_name.is_match(name) {
            push_error(
                errors,
                "http.bearer_token_env".to_string(),
                "invalid_environment_variable",
                "Bearer Token 环境变量名不合法",
            );
        } else if match std::env::var(name) {
            Ok(value) => value.trim().is_empty(),
            Err(_) => true,
        } {
            push_error(
                errors,
                "http.bearer_token_env".to_string(),
                "missing_secret",
                "引用的 Bearer Token 环境变量未设置",
            );
        }
    }
    if let Some(limit) = http.rate_limit.as_ref() {
        if limit.enabled && limit.requests_per_second == 0 {
            push_error(
                errors,
                "http.rate_limit.requests_per_second".to_string(),
                "out_of_range",
                "启用限流时 requests_per_second 必须大于 0",
            );
        }
        if limit.enabled && limit.burst == 0 {
            push_error(
                errors,
                "http.rate_limit.burst".to_string(),
                "out_of_range",
                "启用限流时 burst 必须大于 0",
            );
        }
        for (name, route) in &limit.routes {
            if name.trim().is_empty() {
                push_error(
                    errors,
                    "http.rate_limit.routes".to_string(),
                    "invalid_route_name",
                    "限流路由名称不能为空",
                );
            }
            if route.requests_per_second == 0 {
                push_error(
                    errors,
                    format!("http.rate_limit.routes.{name}.requests_per_second"),
                    "out_of_range",
                    "requests_per_second 必须大于 0",
                );
            }
            if route.burst == 0 {
                push_error(
                    errors,
                    format!("http.rate_limit.routes.{name}.burst"),
                    "out_of_range",
                    "burst 必须大于 0",
                );
            }
        }
    }
    if let Some(dashboard) = http.dashboard.as_ref() {
        if dashboard
            .host
            .as_deref()
            .is_some_and(|host| host.trim().is_empty())
        {
            push_error(
                errors,
                "http.dashboard.host".to_string(),
                "required",
                "Dashboard Host 不能为空",
            );
        }
        if dashboard.port == Some(0) {
            push_error(
                errors,
                "http.dashboard.port".to_string(),
                "out_of_range",
                "Dashboard Port 必须大于 0",
            );
        }
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

    #[test]
    fn rejects_missing_e2b_key_before_daemon_start() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[server.operation_backend]\nkind = \"e2b\"\n[server.operation_backend.options]\napi_key_env = \"XIAOO_TEST_MISSING_E2B_KEY_82\"\n",
        )
        .expect("write config");
        std::env::remove_var("XIAOO_TEST_MISSING_E2B_KEY_82");

        let report = validate_config_file(&path);

        assert!(!report.valid);
        assert!(report.errors.iter().any(|issue| {
            issue.path == "server.operation_backend.options.api_key_env"
                && issue.code == "missing_api_key"
        }));
    }

    #[test]
    fn rejects_missing_channel_secret_before_daemon_start() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::env::remove_var("XIAOO_TEST_MISSING_TELEGRAM_VALIDATION");
        std::fs::write(
            &path,
            "[llm]\nprovider='ollama'\nmodel='test'\n\n[channels.telegram]\nenabled=true\ntransport='polling'\nbot_token_env='XIAOO_TEST_MISSING_TELEGRAM_VALIDATION'\n",
        )
        .expect("write config");

        let report = validate_config_file(&path);

        assert!(!report.valid);
        assert!(report.errors.iter().any(|issue| {
            issue.path == "channels.telegram.bot_token_env" && issue.code == "missing_secret"
        }));
    }

    #[test]
    fn rejects_invalid_http_limits_and_missing_bearer_secret() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        std::env::remove_var("XIAOO_TEST_MISSING_HTTP_TOKEN");
        std::fs::write(
            &path,
            "[llm]\nprovider='ollama'\nmodel='test'\n\n[http]\nbearer_token_env='XIAOO_TEST_MISSING_HTTP_TOKEN'\n\n[http.rate_limit]\nenabled=true\nrequests_per_second=0\nburst=0\n\n[http.dashboard]\nhost=''\nport=0\n",
        )
        .expect("write config");

        let report = validate_config_file(&path);

        assert!(!report.valid);
        for path in [
            "http.bearer_token_env",
            "http.rate_limit.requests_per_second",
            "http.rate_limit.burst",
            "http.dashboard.host",
            "http.dashboard.port",
        ] {
            assert!(
                report.errors.iter().any(|issue| issue.path == path),
                "{path}"
            );
        }
    }
}
