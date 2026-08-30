use serde::Serialize;
use xiaoo_shared::gateway::{
    McpMemoryAutomation, MemoryAutomationConfig, MemoryAutomationError, MemoryAutomationHealth,
};

use crate::daemon_config::DaemonConfig;

#[derive(Debug, Serialize)]
pub struct MemoryAutomationReport {
    pub schema_version: u32,
    pub enabled: bool,
    pub server: String,
    pub connected: bool,
    pub required_tools_available: bool,
    pub health: Option<MemoryAutomationHealth>,
    pub queue_path: String,
    pub pending_ingests: Option<usize>,
    pub queue_capacity: usize,
    pub recall_top_k: usize,
    pub recall_token_budget: usize,
    pub context_messages: usize,
    pub max_retries: u32,
    pub retry_backoff_ms: u64,
    pub allowed_agent_roles: Vec<String>,
    pub success: bool,
    pub error_code: Option<&'static str>,
    pub error: Option<&'static str>,
}

pub async fn memory_automation_report(config: &DaemonConfig) -> MemoryAutomationReport {
    let settings = config.app.memory_automation.clone();
    let mut report = base_report(&settings);
    if !settings.enabled {
        report.success = true;
        return report;
    }

    match McpMemoryAutomation::inspect(&settings, &config.app.mcp.servers).await {
        Ok(Some(inspection)) => {
            report.connected = true;
            report.required_tools_available = true;
            report.health = Some(inspection.health);
            report.pending_ingests = Some(inspection.pending_ingests);
            report.success = true;
        }
        Ok(None) => {
            report.error_code = Some("unexpected_disabled_state");
            report.error = Some("memory automation did not start");
        }
        Err(error) => {
            let (code, message) = safe_error(&error);
            report.error_code = Some(code);
            report.error = Some(message);
        }
    }
    report
}

fn base_report(settings: &MemoryAutomationConfig) -> MemoryAutomationReport {
    MemoryAutomationReport {
        schema_version: 1,
        enabled: settings.enabled,
        server: settings.server.clone(),
        connected: false,
        required_tools_available: false,
        health: None,
        queue_path: settings.queue_path.display().to_string(),
        pending_ingests: None,
        queue_capacity: settings.queue_capacity,
        recall_top_k: settings.recall_top_k,
        recall_token_budget: settings.recall_token_budget,
        context_messages: settings.context_messages,
        max_retries: settings.max_retries,
        retry_backoff_ms: settings.retry_backoff_ms,
        allowed_agent_roles: settings.allowed_agent_roles.clone(),
        success: false,
        error_code: None,
        error: None,
    }
}

fn safe_error(error: &MemoryAutomationError) -> (&'static str, &'static str) {
    match error {
        MemoryAutomationError::Config(_) => (
            "invalid_configuration",
            "memory server is unavailable or lacks required tools",
        ),
        MemoryAutomationError::Mcp(_) => ("mcp_connection_failed", "memory MCP connection failed"),
        MemoryAutomationError::Io(_) => ("queue_io_failed", "memory queue could not be opened"),
        MemoryAutomationError::Json(_) => {
            ("queue_format_invalid", "memory queue contains invalid data")
        }
        MemoryAutomationError::QueueFull => ("queue_full", "memory queue is full"),
        MemoryAutomationError::QueueLocked => ("queue_locked", "memory queue is locked"),
    }
}

#[cfg(test)]
mod tests {
    use super::memory_automation_report;
    use crate::daemon_config::DaemonConfig;

    #[tokio::test]
    async fn disabled_memory_report_does_not_connect() {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[memory_automation]\nenabled = false\nserver = \"missing\"\n",
        )
        .expect("write config");
        let config = DaemonConfig::load_from(&config_path).expect("load config");

        let report = memory_automation_report(&config).await;

        assert!(report.success);
        assert!(!report.connected);
        assert_eq!(report.error_code, None);
    }

    #[tokio::test]
    async fn missing_enabled_memory_server_returns_safe_failure() {
        let temp = tempfile::tempdir().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[memory_automation]\nenabled = true\nserver = \"missing\"\n",
        )
        .expect("write config");
        let config = DaemonConfig::load_from(&config_path).expect("load config");

        let report = memory_automation_report(&config).await;

        assert!(!report.success);
        assert!(!report.connected);
        assert_eq!(report.error_code, Some("invalid_configuration"));
        assert!(!report.error.expect("safe error").contains("missing"));
    }
}
