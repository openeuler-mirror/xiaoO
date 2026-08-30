use serde::Serialize;
use serde_json::Value;
use std::path::Path;

use crate::daemon_config::DaemonConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackendIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct BackendIsolationStatus {
    pub kind: String,
    pub platform_supported: bool,
    pub dependency: Option<String>,
    pub dependency_available: bool,
    pub allow_network: bool,
    pub readable_root_count: usize,
    pub writable_root_count: usize,
}

#[derive(Debug, Serialize)]
pub struct BackendReport {
    pub schema_version: u32,
    pub configured: bool,
    pub kind: String,
    pub supported: bool,
    pub valid: bool,
    pub option_keys: Vec<String>,
    pub api_key_env: Option<String>,
    pub api_key_available: Option<bool>,
    pub api_key_source: Option<&'static str>,
    pub isolation: Option<BackendIsolationStatus>,
    pub capabilities: Vec<&'static str>,
    pub errors: Vec<BackendIssue>,
}

pub fn backend_report(config: &DaemonConfig) -> BackendReport {
    let configured = config.app.server.operation_backend.is_some();
    let kind = config
        .app
        .server
        .operation_backend
        .as_ref()
        .map(|value| value.kind.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("local")
        .to_string();
    let options = config
        .app
        .server
        .operation_backend
        .as_ref()
        .map(|value| &value.options)
        .unwrap_or(&Value::Null);
    let errors = backend_issues(&kind, options);
    let object = options.as_object();
    let mut option_keys = object
        .map(|value| value.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    option_keys.sort();
    let api_key_env = (kind == "e2b").then(|| {
        object
            .and_then(|value| value.get("api_key_env"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("E2B_API_KEY")
            .to_string()
    });
    let inline_api_key = object
        .and_then(|value| value.get("api_key"))
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let environment_api_key = api_key_env.as_deref().is_some_and(|env_name| {
        std::env::var(env_name).is_ok_and(|value| !value.trim().is_empty())
    });
    let api_key_available = (kind == "e2b").then_some(inline_api_key || environment_api_key);
    let api_key_source = (kind == "e2b").then_some(if inline_api_key {
        "inline"
    } else if environment_api_key {
        "environment"
    } else {
        "missing"
    });
    let isolation = (kind == "local")
        .then(|| object.and_then(|value| value.get("isolation")))
        .flatten()
        .and_then(isolation_status);
    BackendReport {
        schema_version: 1,
        configured,
        supported: matches!(kind.as_str(), "local" | "e2b"),
        valid: errors.is_empty(),
        kind,
        option_keys,
        api_key_env,
        api_key_available,
        api_key_source,
        isolation,
        capabilities: vec![
            "exec",
            "read_file",
            "write_file",
            "search",
            "export_file",
            "checkpoint",
            "pause",
            "resume",
        ],
        errors,
    }
}

pub fn backend_issues(kind: &str, options: &Value) -> Vec<BackendIssue> {
    let mut errors = Vec::new();
    if !matches!(kind, "local" | "e2b") {
        issue(
            &mut errors,
            "server.operation_backend.kind",
            "unsupported_backend",
            "daemon 仅支持 local 或 e2b 后端",
        );
    }
    if !options.is_null() && !options.is_object() {
        issue(
            &mut errors,
            "server.operation_backend.options",
            "invalid_type",
            "后端 options 必须是 JSON 对象",
        );
        return errors;
    }
    if kind == "local" {
        if let Some(isolation) = options.get("isolation") {
            validate_isolation(isolation, &mut errors);
        }
    }
    if kind == "e2b" {
        let object = options.as_object();
        let env_name = object
            .and_then(|value| value.get("api_key_env"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("E2B_API_KEY");
        let available = object
            .and_then(|value| value.get("api_key"))
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
            || std::env::var(env_name).is_ok_and(|value| !value.trim().is_empty());
        if !available {
            issue(
                &mut errors,
                "server.operation_backend.options.api_key_env",
                "missing_api_key",
                "E2B API Key 未设置",
            );
        }
    }
    errors
}

fn validate_isolation(value: &Value, errors: &mut Vec<BackendIssue>) {
    let Some(object) = value.as_object() else {
        issue(
            errors,
            "server.operation_backend.options.isolation",
            "invalid_type",
            "隔离配置必须是 JSON 对象",
        );
        return;
    };
    let Some(kind) = object.get("kind").and_then(Value::as_str) else {
        issue(
            errors,
            "server.operation_backend.options.isolation.kind",
            "required",
            "隔离类型不能为空",
        );
        return;
    };
    let (platform_supported, dependency, available) = isolation_environment(kind);
    if !matches!(
        kind,
        "macos_seatbelt" | "linux_bubblewrap" | "linux_dynsandbox"
    ) {
        issue(
            errors,
            "server.operation_backend.options.isolation.kind",
            "unsupported_isolation",
            "不支持的隔离类型",
        );
    } else if !platform_supported {
        issue(
            errors,
            "server.operation_backend.options.isolation.kind",
            "unsupported_platform",
            "当前平台不支持该隔离类型",
        );
    } else if !available {
        issue(
            errors,
            "server.operation_backend.options.isolation.kind",
            "missing_dependency",
            format!("缺少隔离依赖：{}", dependency.unwrap_or("unknown")),
        );
    }
    for field in ["readable_roots", "writable_roots"] {
        if let Some(values) = object.get(field) {
            if !values.as_array().is_some_and(|items| {
                items.iter().all(|item| {
                    item.as_str()
                        .is_some_and(|path| Path::new(path).is_absolute())
                })
            }) {
                issue(
                    errors,
                    &format!("server.operation_backend.options.isolation.{field}"),
                    "invalid_paths",
                    "根目录必须是绝对路径字符串数组",
                );
            }
        }
    }
}

fn isolation_status(value: &Value) -> Option<BackendIsolationStatus> {
    let object = value.as_object()?;
    let kind = object.get("kind")?.as_str()?.to_string();
    let (platform_supported, dependency, dependency_available) = isolation_environment(&kind);
    Some(BackendIsolationStatus {
        kind,
        platform_supported,
        dependency: dependency.map(str::to_string),
        dependency_available,
        allow_network: object
            .get("allow_network")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        readable_root_count: object
            .get("readable_roots")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
        writable_root_count: object
            .get("writable_roots")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
    })
}

fn isolation_environment(kind: &str) -> (bool, Option<&'static str>, bool) {
    match kind {
        "macos_seatbelt" => (
            cfg!(target_os = "macos"),
            Some("sandbox-exec"),
            command_available("sandbox-exec"),
        ),
        "linux_bubblewrap" => (
            cfg!(target_os = "linux"),
            Some("bwrap"),
            command_available("bwrap"),
        ),
        "linux_dynsandbox" => (
            cfg!(target_os = "linux"),
            Some("dyn-sandbox"),
            command_available("dyn-sandbox"),
        ),
        _ => (false, None, false),
    }
}

fn command_available(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(command).is_file())
    })
}

fn issue(errors: &mut Vec<BackendIssue>, path: &str, code: &str, message: impl Into<String>) {
    errors.push(BackendIssue {
        path: path.to_string(),
        code: code.to_string(),
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::backend_report;
    use crate::daemon_config::DaemonConfig;

    #[test]
    fn default_backend_is_local_and_does_not_expose_options() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n").expect("config");
        let report = backend_report(&DaemonConfig::load_from(&path).expect("load"));
        assert_eq!(report.kind, "local");
        assert!(report.valid);
        assert!(report.option_keys.is_empty());
    }

    #[test]
    fn e2b_report_exposes_key_availability_but_not_value() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n[server.operation_backend]\nkind = \"e2b\"\n[server.operation_backend.options]\napi_key = \"must-not-leak\"\ntemplate_id = \"base\"\n").expect("config");
        let report = backend_report(&DaemonConfig::load_from(&path).expect("load"));
        let json = serde_json::to_string(&report).expect("json");
        assert_eq!(report.api_key_available, Some(true));
        assert_eq!(report.api_key_source, Some("inline"));
        assert!(!json.contains("must-not-leak"));
    }
}
