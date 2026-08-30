use serde::Serialize;
use xiaoo_api::llm::{
    DEFAULT_AUTO_COMPACT_RATIO, DEFAULT_BLOCKING_RATIO, DEFAULT_COLLAPSE_PRESERVE_TAIL,
    DEFAULT_SNIP_PRESERVE_TAIL, DEFAULT_SNIP_STALE_AFTER_MS, DEFAULT_SUMMARY_LLM_MAX_TOKENS,
    DEFAULT_SUMMARY_MAX_TOKENS, DEFAULT_SUMMARY_PRESERVE_TAIL, DEFAULT_WARNING_RATIO,
};

use crate::daemon_config::{CompactConfig, DaemonConfig};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CompactIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct EffectiveCompactConfig {
    pub warning_ratio: f64,
    pub auto_compact_ratio: f64,
    pub blocking_ratio: f64,
    pub snip_stale_after_ms: u64,
    pub snip_preserve_tail: usize,
    pub collapse_preserve_tail: usize,
    pub summary_max_tokens: usize,
    pub summary_preserve_tail: usize,
    pub summary_llm_max_tokens: usize,
}

#[derive(Debug, Serialize)]
pub struct CompactReport {
    pub schema_version: u32,
    pub configured: bool,
    pub valid: bool,
    pub configured_fields: Vec<&'static str>,
    pub effective: EffectiveCompactConfig,
    pub errors: Vec<CompactIssue>,
}

pub fn compact_report(config: &DaemonConfig) -> CompactReport {
    let compact = config.app.compact.as_ref();
    let errors = compact.map(compact_issues).unwrap_or_default();
    CompactReport {
        schema_version: 1,
        configured: compact.is_some(),
        valid: errors.is_empty(),
        configured_fields: compact.map(configured_fields).unwrap_or_default(),
        effective: effective_config(compact),
        errors,
    }
}

pub fn compact_issues(config: &CompactConfig) -> Vec<CompactIssue> {
    let effective = effective_config(Some(config));
    let mut errors = Vec::new();
    for (name, ratio) in [
        ("warning_ratio", effective.warning_ratio),
        ("auto_compact_ratio", effective.auto_compact_ratio),
        ("blocking_ratio", effective.blocking_ratio),
    ] {
        if !(ratio > 0.0 && ratio <= 1.0) {
            issue(
                &mut errors,
                &format!("compact.{name}"),
                "out_of_range",
                "压缩比例必须大于 0 且不大于 1",
            );
        }
    }
    if !(effective.warning_ratio <= effective.auto_compact_ratio
        && effective.auto_compact_ratio <= effective.blocking_ratio)
    {
        issue(
            &mut errors,
            "compact",
            "invalid_threshold_order",
            "压缩阈值必须满足 warning_ratio ≤ auto_compact_ratio ≤ blocking_ratio",
        );
    }
    for (name, value) in [
        ("snip_preserve_tail", effective.snip_preserve_tail),
        ("collapse_preserve_tail", effective.collapse_preserve_tail),
        ("summary_max_tokens", effective.summary_max_tokens),
        ("summary_preserve_tail", effective.summary_preserve_tail),
        ("summary_llm_max_tokens", effective.summary_llm_max_tokens),
    ] {
        if value == 0 {
            issue(
                &mut errors,
                &format!("compact.{name}"),
                "out_of_range",
                "该值必须大于 0",
            );
        }
    }
    errors
}

fn effective_config(config: Option<&CompactConfig>) -> EffectiveCompactConfig {
    EffectiveCompactConfig {
        warning_ratio: config
            .and_then(|value| value.warning_ratio)
            .unwrap_or(DEFAULT_WARNING_RATIO),
        auto_compact_ratio: config
            .and_then(|value| value.auto_compact_ratio)
            .unwrap_or(DEFAULT_AUTO_COMPACT_RATIO),
        blocking_ratio: config
            .and_then(|value| value.blocking_ratio)
            .unwrap_or(DEFAULT_BLOCKING_RATIO),
        snip_stale_after_ms: config
            .and_then(|value| value.snip_stale_after_ms)
            .unwrap_or(DEFAULT_SNIP_STALE_AFTER_MS),
        snip_preserve_tail: config
            .and_then(|value| value.snip_preserve_tail)
            .unwrap_or(DEFAULT_SNIP_PRESERVE_TAIL),
        collapse_preserve_tail: config
            .and_then(|value| value.collapse_preserve_tail)
            .unwrap_or(DEFAULT_COLLAPSE_PRESERVE_TAIL),
        summary_max_tokens: config
            .and_then(|value| value.summary_max_tokens)
            .unwrap_or(DEFAULT_SUMMARY_MAX_TOKENS),
        summary_preserve_tail: config
            .and_then(|value| value.summary_preserve_tail)
            .unwrap_or(DEFAULT_SUMMARY_PRESERVE_TAIL),
        summary_llm_max_tokens: config
            .and_then(|value| value.summary_llm_max_tokens)
            .unwrap_or(DEFAULT_SUMMARY_LLM_MAX_TOKENS),
    }
}

fn configured_fields(config: &CompactConfig) -> Vec<&'static str> {
    let mut fields = Vec::new();
    for (name, configured) in [
        ("warning_ratio", config.warning_ratio.is_some()),
        ("auto_compact_ratio", config.auto_compact_ratio.is_some()),
        ("blocking_ratio", config.blocking_ratio.is_some()),
        ("snip_stale_after_ms", config.snip_stale_after_ms.is_some()),
        ("snip_preserve_tail", config.snip_preserve_tail.is_some()),
        (
            "collapse_preserve_tail",
            config.collapse_preserve_tail.is_some(),
        ),
        ("summary_max_tokens", config.summary_max_tokens.is_some()),
        (
            "summary_preserve_tail",
            config.summary_preserve_tail.is_some(),
        ),
        (
            "summary_llm_max_tokens",
            config.summary_llm_max_tokens.is_some(),
        ),
    ] {
        if configured {
            fields.push(name);
        }
    }
    fields
}

fn issue(errors: &mut Vec<CompactIssue>, path: &str, code: &str, message: impl Into<String>) {
    errors.push(CompactIssue {
        path: path.to_string(),
        code: code.to_string(),
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::compact_report;
    use crate::daemon_config::DaemonConfig;

    #[test]
    fn reports_effective_defaults_without_a_compact_section() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n")
            .expect("write config");
        let report = compact_report(&DaemonConfig::load_from(&path).expect("config"));
        assert!(!report.configured);
        assert!(report.valid);
        assert_eq!(report.effective.warning_ratio, 0.6);
        assert_eq!(report.effective.blocking_ratio, 0.9);
    }

    #[test]
    fn rejects_invalid_threshold_order_and_zero_budgets() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[compact]\nwarning_ratio = 0.8\nauto_compact_ratio = 0.5\nblocking_ratio = 0.9\nsummary_max_tokens = 0\n",
        )
        .expect("write config");
        let report = compact_report(&DaemonConfig::load_from(&path).expect("config"));
        assert!(!report.valid);
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.code == "invalid_threshold_order"));
        assert!(report
            .errors
            .iter()
            .any(|issue| issue.path == "compact.summary_max_tokens"));
    }
}
