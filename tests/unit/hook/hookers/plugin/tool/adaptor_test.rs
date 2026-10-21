use super::*;
use std::path::PathBuf;

use super::super::super::interaction::{
    parse_plugin_command_response, AskUserDirective, PluginAskUserRequest, PluginCommandResponse,
};
use super::super::super::test_support::{TestAgentContext, TestRuntimeView};

use agent_types::hook::HookInvokePrimary;
use agent_types::interaction::types::InteractionSource;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::tool::FinalToolCall;

// The shared scaffold (TestRuntimeView / TestAgentContext and friends)
// lives in `test_support.rs`, mounted at `crates/hook/src/hookers/plugin/mod.rs`.

#[tokio::test]
async fn ask_user_round_trip_returns_final_pre_result() {
    let script_path = std::env::temp_dir().join(format!(
        "plugin_ask_user_round_trip_{}.py",
        std::process::id()
    ));
    std::fs::write(
        &script_path,
        r#"import json
import sys

payload = json.load(sys.stdin)
interaction = payload.get("interaction")
if interaction is None:
    json.dump(
        {
            "action": "ask_user",
            "request": {
                "kind": "text_input",
                "prompt": "who approved this?",
            },
            "continuation": {"step": "final"},
        },
        sys.stdout,
    )
else:
    value = interaction["response"].get("value")
    json.dump(
        {
            "action": "final",
            "result": "deny",
            "reason": f"approved by {value}",
        },
        sys.stdout,
    )
"#,
    )
    .unwrap();

    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        format!("python3 {}", script_path.display()),
        Value::Null,
    );
    let runtime = TestRuntimeView::with_interaction_response(InteractionResponse::Text {
        value: Some("alice".to_string()),
        display_value: None,
    });
    let input = PreToolHookInput {
        call: FinalToolCall {
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({"command": "pwd"}),
            ..Default::default()
        },
    };

    let output = adaptor
        .invoke_pre(&input, &HookInvokeMetadata::default(), &runtime)
        .await
        .unwrap();

    match output.primary {
        HookInvokePrimary::Pre(PreHookResult::Deny { reason }) => {
            assert_eq!(reason, "approved by alice");
        }
        other => panic!("unexpected output: {:?}", other),
    }

    let recorded_requests = runtime.interaction.recorded_requests();
    assert_eq!(recorded_requests.len(), 1);
    match &recorded_requests[0] {
        InteractionRequest::TextInput {
            prompt,
            source:
                Some(InteractionSource::Hooker {
                    hooker_name,
                    hook_point,
                }),
            is_secret: _, // Ignore is_secret in test
        } => {
            assert_eq!(prompt, "who approved this?");
            assert_eq!(hooker_name, "plugin_pre_ask");
            assert_eq!(hook_point, "test-agent.Tool.bash.pre");
        }
        other => panic!("unexpected interaction request: {:?}", other),
    }

    let _ = std::fs::remove_file(script_path);
}

#[test]
fn ask_user_requires_continuation_field() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    let error = parse_plugin_command_response(
        adaptor.core.id(),
        json!({
            "action": "ask_user",
            "request": {
                "kind": "confirm",
                "prompt": "continue?",
                "source": null,
            }
        }),
        &PluginToolHookerAdaptor::err,
    )
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("response must contain field 'continuation'"));
}

#[test]
fn ask_user_request_parses_minimal_confirm() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    let parsed = parse_plugin_command_response(
        adaptor.core.id(),
        json!({
            "action": "ask_user",
            "request": {
                "kind": "confirm",
                "prompt": "continue?"
            },
            "continuation": {"step": 1}
        }),
        &PluginToolHookerAdaptor::err,
    )
    .unwrap();

    match parsed {
        PluginCommandResponse::AskUser(AskUserDirective {
            request: PluginAskUserRequest::Confirm { prompt, .. },
            continuation,
        }) => {
            assert_eq!(prompt, "continue?");
            assert_eq!(continuation, json!({"step": 1}));
        }
        other => panic!("unexpected parsed response: {:?}", other),
    }
}

#[test]
fn final_responses_remain_backward_compatible() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_ask".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    let legacy = parse_plugin_command_response(
        adaptor.core.id(),
        json!({
            "result": "allow"
        }),
        &PluginToolHookerAdaptor::err,
    )
    .unwrap();
    let explicit = parse_plugin_command_response(
        adaptor.core.id(),
        json!({
            "action": "final",
            "result": "allow"
        }),
        &PluginToolHookerAdaptor::err,
    )
    .unwrap();

    assert!(matches!(legacy, PluginCommandResponse::Final(_)));
    assert!(matches!(explicit, PluginCommandResponse::Final(_)));
}

#[test]
fn allow_result_parses_optional_extra() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_allow".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "python3 unused.py".to_string(),
        Value::Null,
    );

    // Legacy plugins: plain allow carries no extra.
    match adaptor
        .parse_pre_result(&json!({ "result": "allow" }))
        .unwrap()
    {
        PreHookResult::Allow { extra } => assert!(extra.is_none()),
        other => panic!("unexpected result: {:?}", other),
    }

    // allow + extra: the payload is surfaced verbatim for accumulation.
    match adaptor
        .parse_pre_result(&json!({
            "result": "allow",
            "extra": { "spawn_wrapper": { "prefix": ["/bin/true"] } }
        }))
        .unwrap()
    {
        PreHookResult::Allow { extra } => {
            assert_eq!(
                extra,
                Some(json!({ "spawn_wrapper": { "prefix": ["/bin/true"] } }))
            );
        }
        other => panic!("unexpected result: {:?}", other),
    }

    // deny ignores an extra key entirely.
    match adaptor
        .parse_pre_result(&json!({
            "result": "deny",
            "reason": "not allowed",
            "extra": { "ignored": true }
        }))
        .unwrap()
    {
        PreHookResult::Deny { reason } => assert_eq!(reason, "not allowed"),
        other => panic!("unexpected result: {:?}", other),
    }
}

/// End-to-end pre invocation against a plugin script whose final response is
/// `allow` with an `extra` payload: stdout JSON must reach the caller as
/// `PreHookResult::Allow { extra: Some(..) }` so the framework can accumulate
/// it into `FinalToolCall.extra`.
#[tokio::test]
async fn invoke_pre_returns_allow_with_extra_from_plugin_stdout() {
    let script_path =
        std::env::temp_dir().join(format!("plugin_allow_extra_{}.py", std::process::id()));
    std::fs::write(
        &script_path,
        r#"import json
import sys

json.load(sys.stdin)
json.dump(
    {
        "result": "allow",
        "extra": {"spawn_wrapper": {"prefix": ["/opt/wrapper", "--session=s1", "--"]}},
    },
    sys.stdout,
)
"#,
    )
    .unwrap();

    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_pre_allow".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        format!("python3 {}", script_path.display()),
        Value::Null,
    );
    let runtime = TestRuntimeView::with_interaction_response(InteractionResponse::Text {
        value: None,
        display_value: None,
    });
    let input = PreToolHookInput {
        call: FinalToolCall {
            call_id: "call-1".to_string(),
            tool_name: "bash".to_string(),
            input: json!({"command": "pwd"}),
            ..Default::default()
        },
    };

    let output = adaptor
        .invoke_pre(&input, &HookInvokeMetadata::default(), &runtime)
        .await
        .unwrap();

    match output.primary {
        HookInvokePrimary::Pre(PreHookResult::Allow { extra }) => {
            assert_eq!(
                extra,
                Some(json!({
                    "spawn_wrapper": {"prefix": ["/opt/wrapper", "--session=s1", "--"]}
                }))
            );
        }
        other => panic!("unexpected output: {:?}", other),
    }

    let _ = std::fs::remove_file(script_path);
}

/// Verify the plugin hooker child-process timeout + kill fallback chain:
/// spawn a `sleep 30` child (simulating a hang) with a 2s timeout, and assert:
/// 1. It returns ToolExecutionError::Timeout (instead of blocking forever);
/// 2. The elapsed time is near the timeout (~2s), proving the timeout fired
///    rather than the child exiting on its own;
/// 3. kill_on_drop kills the child after the future is dropped (no leftover
///    process check; guaranteed by kill_on_drop semantics).
#[tokio::test]
async fn run_plugin_command_times_out_and_kills_hung_child() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_timeout".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "sleep 30".to_string(),
        Value::Null,
    );
    let payload = json!({"stage": "pre"});

    let start = std::time::Instant::now();
    let result = adaptor
        .core
        .run_plugin_command(
            &payload,
            &PluginToolHookerAdaptor::err,
            &PluginToolHookerAdaptor::timeout_err,
            Some(2_000),
        )
        .await;
    let elapsed = start.elapsed();

    // 1. Returns a Timeout error (not Ok, not other ExecutionFailed)
    match result {
        Err(ToolExecutionError::Timeout { timeout_ms }) => {
            assert_eq!(timeout_ms, 2000, "timeout_ms 应等于传入超时(2s=2000ms)");
        }
        other => panic!("expected Timeout, got: {:?}", other),
    }

    // 2. Elapsed time should be near the timeout (2s), allowing scheduling
    //    jitter, but far below the 30s of `sleep 30`.
    //    Upper bound 10s: exceeding it means the child was never killed by
    //    the timeout (still stuck).
    assert!(
        elapsed.as_secs() < 10,
        "超时应在 ~2s 触发，实际耗时 {:?}（疑似未被超时 kill）",
        elapsed
    );
    // Lower bound: must wait at least until the timeout threshold; an instant
    // return means the child never hung.
    assert!(
        elapsed.as_secs() >= 2,
        "应等到 2s 超时才返回，实际耗时 {:?}（疑似子进程未卡住）",
        elapsed
    );
}

// ---- payload identity fields (workspace / session_id) -----------------

fn sample_call() -> FinalToolCall {
    FinalToolCall {
        call_id: "call-9".to_string(),
        tool_name: "bash".to_string(),
        input: json!({"command": "pwd"}),
        ..Default::default()
    }
}

#[test]
fn pre_payload_carries_workspace_and_session_id() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_payload_test".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "cat > /dev/null".to_string(),
        Value::Null,
    );
    let runtime =
        TestRuntimeView::with_agent_context(TestAgentContext::with_workspace_and_session(
            PathBuf::from("/ws/proj-a"),
            Some("session-42".to_string()),
        ));
    let input = PreToolHookInput {
        call: sample_call(),
    };

    let payload = adaptor
        .build_pre_payload(&input, &HookInvokeMetadata::default(), &runtime)
        .unwrap();

    assert_eq!(payload["stage"], json!("pre"));
    assert_eq!(payload["session_id"], json!("session-42"));
    assert_eq!(payload["workspace"], json!("/ws/proj-a"));
}

#[test]
fn post_payload_carries_session_id_and_workspace() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_payload_test".to_string()),
        HookPointId("test-agent.Tool.bash.post".to_string()),
        "cat > /dev/null".to_string(),
        Value::Null,
    );
    let runtime =
        TestRuntimeView::with_agent_context(TestAgentContext::with_workspace_and_session(
            PathBuf::from("/ws/proj-a"),
            Some("session-42".to_string()),
        ));
    let input = PostToolHookInput {
        call: sample_call(),
        outcome: RawToolOutcome::Success {
            output: "ok".to_string(),
        },
    };

    let payload = adaptor
        .build_post_payload(&input, &HookInvokeMetadata::default(), &runtime)
        .unwrap();

    assert_eq!(payload["stage"], json!("post"));
    assert_eq!(payload["session_id"], json!("session-42"));
    assert_eq!(payload["workspace"], json!("/ws/proj-a"));
    assert_eq!(payload["outcome"]["type"], json!("success"));
}

#[test]
fn error_payload_carries_session_id_and_workspace() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_payload_test".to_string()),
        HookPointId("test-agent.Tool.bash.error".to_string()),
        "cat > /dev/null".to_string(),
        Value::Null,
    );
    let runtime =
        TestRuntimeView::with_agent_context(TestAgentContext::with_workspace_and_session(
            PathBuf::from("/ws/proj-a"),
            Some("session-42".to_string()),
        ));
    let input = ErrorToolHookInput {
        call: sample_call(),
        error: ToolExecutionError::ExecutionFailed {
            message: "boom".to_string(),
        },
    };

    let payload = adaptor
        .build_error_payload(&input, &HookInvokeMetadata::default(), &runtime)
        .unwrap();

    assert_eq!(payload["stage"], json!("error"));
    assert_eq!(payload["session_id"], json!("session-42"));
    assert_eq!(payload["workspace"], json!("/ws/proj-a"));
    assert_eq!(payload["error"]["type"], json!("execution_failed"));
}

#[test]
fn post_and_error_session_id_falls_back_to_call_id() {
    // Same fallback rule as the pre payload: without a runtime session id
    // the call id identifies the invocation.
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_payload_test".to_string()),
        HookPointId("test-agent.Tool.bash.post".to_string()),
        "cat > /dev/null".to_string(),
        Value::Null,
    );
    let runtime = TestRuntimeView::with_agent_context(
        TestAgentContext::with_workspace_and_session(PathBuf::from("/ws/proj-a"), None),
    );
    let post_input = PostToolHookInput {
        call: sample_call(),
        outcome: RawToolOutcome::Success {
            output: String::new(),
        },
    };
    let payload = adaptor
        .build_post_payload(&post_input, &HookInvokeMetadata::default(), &runtime)
        .unwrap();
    assert_eq!(payload["session_id"], json!("call-9"));

    let error_input = ErrorToolHookInput {
        call: sample_call(),
        error: ToolExecutionError::ExecutionFailed {
            message: "boom".to_string(),
        },
    };
    let payload = adaptor
        .build_error_payload(&error_input, &HookInvokeMetadata::default(), &runtime)
        .unwrap();
    assert_eq!(payload["session_id"], json!("call-9"));
}

#[test]
fn payload_workspace_serializes_empty_root_as_null() {
    let adaptor = PluginToolHookerAdaptor::new(
        HookerId("plugin_payload_test".to_string()),
        HookPointId("test-agent.Tool.bash.pre".to_string()),
        "cat > /dev/null".to_string(),
        Value::Null,
    );
    let runtime =
        TestRuntimeView::with_agent_context(TestAgentContext::with_workspace_and_session(
            PathBuf::new(),
            Some("session-42".to_string()),
        ));
    let input = PreToolHookInput {
        call: sample_call(),
    };

    let payload = adaptor
        .build_pre_payload(&input, &HookInvokeMetadata::default(), &runtime)
        .unwrap();

    assert_eq!(payload["workspace"], Value::Null);
}
