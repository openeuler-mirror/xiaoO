use super::*;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_load_subagent_config() {
    let config_content = r#"
[llm]
provider = "anthropic"
model = "claude-sonnet-4-20250514"

[subagent.code_reviewer]
description = "Code review specialist"
prompt = "You are a code review specialist."
max_turns = 5

[subagent.test_writer]
description = "Test writing specialist"
prompt = "You are a test writing specialist."
max_turns = 8
"#;

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config = FileConfig::load_from_path(temp_file.path(), false);

    assert_eq!(config.subagent.len(), 2);
    assert!(config.subagent.contains_key("code_reviewer"));
    assert!(config.subagent.contains_key("test_writer"));

    let reviewer = config.subagent.get("code_reviewer").unwrap();
    assert_eq!(reviewer.description, "Code review specialist");
    assert_eq!(
        reviewer.prompt,
        Some("You are a code review specialist.".to_string())
    );
    assert_eq!(reviewer.max_turns, Some(5));

    let writer = config.subagent.get("test_writer").unwrap();
    assert_eq!(writer.description, "Test writing specialist");
    assert_eq!(writer.max_turns, Some(8));
}

#[test]
fn test_subagent_tools_config() {
    let config_content = r#"
[llm]
provider = "openai"

[subagent.limited_agent]
description = "Agent with limited tools"
tools = { "bash" = true, "read" = true }
"#;

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config = FileConfig::load_from_path(temp_file.path(), false);

    assert_eq!(config.subagent.len(), 1);
    let agent = config.subagent.get("limited_agent").unwrap();
    assert_eq!(agent.tools.len(), 2);
    assert_eq!(agent.tools.get("bash"), Some(&true));
    assert_eq!(agent.tools.get("read"), Some(&true));
    assert_eq!(agent.tools.get("write"), None);
}

#[test]
fn test_empty_subagent_config() {
    let config_content = r#"
[llm]
provider = "anthropic"
"#;

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config = FileConfig::load_from_path(temp_file.path(), false);
    assert_eq!(config.subagent.len(), 0);
}

#[test]
fn test_loads_memory_automation_config() {
    let config_content = r#"
[memory_automation]
enabled = true
server = "ram-a"
recall_top_k = 3
recall_token_budget = 128
context_messages = 2
queue_path = "/tmp/xiaoo-memory-queue.jsonl"
queue_capacity = 32
max_retries = 4
retry_backoff_ms = 50
allowed_agent_roles = ["main", "researcher"]
"#;

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config = FileConfig::load_from_path(temp_file.path(), false);

    assert!(config.memory_automation.enabled);
    assert_eq!(config.memory_automation.server, "ram-a");
    assert_eq!(config.memory_automation.recall_top_k, 3);
    assert_eq!(config.memory_automation.recall_token_budget, 128);
    assert_eq!(config.memory_automation.context_messages, 2);
    assert_eq!(config.memory_automation.queue_capacity, 32);
    assert_eq!(config.memory_automation.max_retries, 4);
    assert_eq!(config.memory_automation.retry_backoff_ms, 50);
    assert_eq!(
        config.memory_automation.allowed_agent_roles,
        vec!["main".to_string(), "researcher".to_string()]
    );
}

#[test]
fn test_load_linux_dynsandbox_operation_backend_config() {
    let config_content = r#"
[llm]
provider = "openai"

[operation_backend]
kind = "local"

[operation_backend.options.isolation]
kind = "linux_dynsandbox"
allow_network = false
readable_roots = ["/home/alice/project"]
writable_roots = ["/home/alice/project/.xiaoo-tmp"]
"#;

    let mut temp_file = NamedTempFile::new().unwrap();
    temp_file.write_all(config_content.as_bytes()).unwrap();
    temp_file.flush().unwrap();

    let config = FileConfig::load_from_path(temp_file.path(), false);

    let backend = config.operation_backend.expect("operation_backend");
    assert_eq!(backend.kind, "local");
    assert_eq!(backend.options["isolation"]["kind"], "linux_dynsandbox");
    assert_eq!(backend.options["isolation"]["allow_network"], false);
    assert_eq!(
        backend.options["isolation"]["readable_roots"][0],
        "/home/alice/project"
    );
    assert_eq!(
        backend.options["isolation"]["writable_roots"][0],
        "/home/alice/project/.xiaoo-tmp"
    );
}
