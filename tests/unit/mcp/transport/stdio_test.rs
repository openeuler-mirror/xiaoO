//! Unit tests for the stdio MCP transport's environment construction.
//!
//! These cover the isolation contract directly: a stdio server must not
//! inherit the daemon's provider credentials, but it must still be able to
//! resolve its own interpreter/executable (PATH) and be opted into extra
//! variables explicitly.

use std::collections::HashMap;

use super::child_env;

fn parent_env() -> Vec<(String, String)> {
    vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("HOME".to_string(), "/root".to_string()),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        // Credentials injected into the daemon environment by
        // `inject_llm_secrets_into_env`, plus an unrelated host variable.
        ("ANTHROPIC_API_KEY".to_string(), "sk-ant-secret".to_string()),
        ("XIAOO_TEST_TOKEN".to_string(), "top-secret".to_string()),
        ("SSH_AUTH_SOCK".to_string(), "/tmp/agent.sock".to_string()),
    ]
}

#[test]
fn stdio_child_does_not_inherit_provider_credentials() {
    let env = child_env(parent_env(), &HashMap::new());
    let names: Vec<&str> = env.iter().map(|(key, _)| key.as_str()).collect();

    assert!(
        !names.contains(&"ANTHROPIC_API_KEY"),
        "provider API keys must not reach a stdio MCP server: {names:?}"
    );
    assert!(
        !names.contains(&"XIAOO_TEST_TOKEN"),
        "non-allowlisted host variables must not reach a stdio MCP server: {names:?}"
    );
    assert!(
        !names.contains(&"SSH_AUTH_SOCK"),
        "the ssh agent socket must not reach a stdio MCP server: {names:?}"
    );
}

#[test]
fn stdio_child_keeps_only_the_launcher_allowlist() {
    let env = child_env(parent_env(), &HashMap::new());
    let mut pairs: Vec<(String, String)> = env;
    pairs.sort();

    assert_eq!(
        pairs,
        vec![
            ("HOME".to_string(), "/root".to_string()),
            ("LANG".to_string(), "C.UTF-8".to_string()),
            ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ],
        "only PATH/HOME/LANG/LC_ALL/TMPDIR may be inherited from the host"
    );
}

#[test]
fn configured_env_is_applied_and_wins_over_the_allowlist() {
    let configured = HashMap::from([
        ("PATH".to_string(), "/custom/bin".to_string()),
        ("MCP_SERVER_TOKEN".to_string(), "configured".to_string()),
    ]);

    let env = child_env(parent_env(), &configured);

    let lookup = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };

    assert_eq!(lookup("PATH"), Some("/custom/bin"));
    assert_eq!(lookup("MCP_SERVER_TOKEN"), Some("configured"));
    // A configured entry must not duplicate a key inherited from the host.
    assert_eq!(env.iter().filter(|(key, _)| key == "PATH").count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn expired_ready_admission_does_not_start_stdio_transport() {
    use std::future::Future;
    let writer = process_coordination::write_permit().await;
    let args = Vec::new();
    let environment = HashMap::new();
    let mut call = Box::pin(super::StdioTransport::spawn(
        "/bin/true",
        &args,
        &environment,
        20,
    ));
    std::future::poll_fn(|cx| {
        assert!(call.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    std::thread::sleep(std::time::Duration::from_millis(30));
    drop(writer);
    assert!(matches!(
        call.await,
        Err(crate::error::McpError::Timeout { timeout_ms: 20 })
    ));
}
