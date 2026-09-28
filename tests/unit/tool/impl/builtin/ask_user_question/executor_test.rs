use std::future::Future;
use std::sync::Mutex;

use agent_contracts::{
    AgentContext, HookerRegistry, InteractionHandle, ToolEventSink, ToolStateStore, TraceRecorder,
};
use serde_json::json;

use super::*;

/// Runs a future on a minimal current-thread runtime; equivalent to
/// `#[tokio::test]`, which needs tokio's `macros` feature (declared for
/// this crate in its `[dev-dependencies]`, so `#[tokio::test]` never
/// depends on a transitive dependency enabling it). Kept for the tests
/// above that predate `#[tokio::test]` usage in this file.
fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for test")
        .block_on(future)
}

/// Records every `ask` call and answers with a fixed choice value.
#[derive(Default)]
struct RecordingInteraction {
    requests: Mutex<Vec<InteractionRequest>>,
}

#[async_trait]
impl InteractionHandle for RecordingInteraction {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        self.requests
            .lock()
            .expect("interaction recorder lock poisoned")
            .push(request.clone());
        InteractionResponse::Choice {
            value: Some("a".to_string()),
        }
    }
}

struct AskRuntime {
    interaction: RecordingInteraction,
}

impl RuntimeView for AskRuntime {
    fn state_store(&self) -> &dyn ToolStateStore {
        panic!("not used in ask_user_question executor tests")
    }

    fn tool_events(&self) -> &dyn ToolEventSink {
        panic!("not used in ask_user_question executor tests")
    }

    fn trace_recorder(&self) -> &dyn TraceRecorder {
        panic!("not used in ask_user_question executor tests")
    }

    fn agent_context(&self) -> &dyn AgentContext {
        panic!("not used in ask_user_question executor tests")
    }

    fn interaction(&self) -> &dyn InteractionHandle {
        &self.interaction
    }

    fn hookers(&self) -> &dyn HookerRegistry {
        panic!("not used in ask_user_question executor tests")
    }
}

fn choice_call(allow_custom_input: bool) -> FinalToolCall {
    FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "ask_user_question".to_string(),
        input: serde_json::json!({
            "questions": [{
                "kind": "choice",
                "prompt": "Pick one",
                "options": ["a", "b"],
                "allow_custom_input": allow_custom_input
            }]
        }),
        extra: None,
    }
}

fn invoke_and_take_requests(call: &FinalToolCall) -> Vec<InteractionRequest> {
    let runtime = AskRuntime {
        interaction: RecordingInteraction::default(),
    };
    let executor = AskUserQuestionExecutor::new(Arc::new(AskUserQuestionToolSpec::new()));
    let output =
        block_on(executor.invoke(call, &runtime)).expect("ask_user_question invoke should succeed");
    match output {
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { .. },
        } => {}
        other => panic!("expected successful completion, got {other:?}"),
    }
    let requests = std::mem::take(&mut *runtime.interaction.requests.lock().unwrap());
    requests
}

#[test]
fn choice_request_always_allows_custom_input() {
    // The TUI convert layer maps this flag faithfully and relies on this
    // guarantee to keep the custom-answer box available.
    for declared in [false, true] {
        let requests = invoke_and_take_requests(&choice_call(declared));
        let request = requests
            .first()
            .expect("executor should have asked one question");
        match request {
            InteractionRequest::Choice {
                allow_custom_input,
                source,
                ..
            } => {
                assert!(
                    *allow_custom_input,
                    "custom input must stay allowed even when the model declared {declared}"
                );
                assert!(matches!(
                    source,
                    Some(InteractionSource::Tool { tool_name }) if tool_name == "ask_user_question"
                ));
            }
            other => panic!("expected a choice interaction request, got {other:?}"),
        }
    }
}

struct ScriptedInteraction {
    requests: Mutex<Vec<InteractionRequest>>,
    responses: Mutex<Vec<InteractionResponse>>,
}

impl ScriptedInteraction {
    fn new(responses: Vec<InteractionResponse>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(responses),
        }
    }

    fn captured_requests(&self) -> Vec<InteractionRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl InteractionHandle for ScriptedInteraction {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        self.requests.lock().unwrap().push(request.clone());
        self.responses.lock().unwrap().remove(0)
    }
}

/// Same as [`AskRuntime`] but backed by a [`ScriptedInteraction`] that
/// replays canned responses, so confirm/text answers can be exercised.
struct ScriptedAskRuntime {
    interaction: ScriptedInteraction,
}

impl RuntimeView for ScriptedAskRuntime {
    fn state_store(&self) -> &dyn ToolStateStore {
        panic!("not used in ask_user_question tests")
    }

    fn tool_events(&self) -> &dyn ToolEventSink {
        panic!("not used in ask_user_question tests")
    }

    fn trace_recorder(&self) -> &dyn TraceRecorder {
        panic!("not used in ask_user_question tests")
    }

    fn agent_context(&self) -> &dyn AgentContext {
        panic!("not used in ask_user_question tests")
    }

    fn interaction(&self) -> &dyn InteractionHandle {
        &self.interaction
    }

    fn hookers(&self) -> &dyn HookerRegistry {
        panic!("not used in ask_user_question tests")
    }
}

async fn invoke(
    input: serde_json::Value,
    responses: Vec<InteractionResponse>,
) -> (String, Vec<InteractionRequest>) {
    let executor =
        AskUserQuestionExecutor::new(std::sync::Arc::new(AskUserQuestionToolSpec::new()));
    let runtime = ScriptedAskRuntime {
        interaction: ScriptedInteraction::new(responses),
    };
    let call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "ask_user_question".to_string(),
        input,
        extra: None,
    };
    let output = executor
        .invoke(&call, &runtime)
        .await
        .expect("ask_user_question execution");
    let ToolExecutorOutput::Completed {
        raw_outcome: RawToolOutcome::Success { output },
    } = output
    else {
        panic!("expected successful completion, got {output:?}");
    };
    (output, runtime.interaction.captured_requests())
}

#[tokio::test]
async fn confirm_denial_is_reported_as_explicit_no() {
    let prompt = "是否恢复该历史任务，从「需求分析」步骤继续？";
    let (output, requests) = invoke(
        json!({ "questions": [ { "kind": "confirm", "prompt": prompt } ] }),
        vec![InteractionResponse::Confirmed { allowed: false }],
    )
    .await;

    assert_eq!(requests.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed,
        json!({
            "answers": [
                { "kind": "confirm", "prompt": prompt, "answer": "no" }
            ]
        })
    );
    assert!(parsed["answers"][0].get("allowed").is_none());
}

#[tokio::test]
async fn confirm_acceptance_is_reported_as_explicit_yes() {
    let (output, _) = invoke(
        json!({ "questions": [ { "kind": "confirm", "prompt": "继续吗？" } ] }),
        vec![InteractionResponse::Confirmed { allowed: true }],
    )
    .await;

    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed,
        json!({
            "answers": [
                { "kind": "confirm", "prompt": "继续吗？", "answer": "yes" }
            ]
        })
    );
}

#[tokio::test]
async fn choice_options_pass_through_to_the_user() {
    let (output, requests) = invoke(
        json!({
            "questions": [
                {
                    "kind": "choice",
                    "prompt": "选择处理方式：",
                    "options": ["恢复历史任务", "作为全新任务开始"],
                    "allow_custom_input": false
                }
            ]
        }),
        vec![InteractionResponse::Choice {
            value: Some("作为全新任务开始".to_string()),
        }],
    )
    .await;

    assert_eq!(requests.len(), 1);
    match &requests[0] {
        InteractionRequest::Choice { options, .. } => {
            assert_eq!(
                options,
                &["恢复历史任务", "作为全新任务开始"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            );
        }
        other => panic!("expected a choice request, got {other:?}"),
    }

    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed,
        json!({
            "answers": [
                { "kind": "choice", "prompt": "选择处理方式：", "value": "作为全新任务开始" }
            ]
        })
    );
}

#[tokio::test]
async fn multiple_questions_keep_input_order() {
    let (output, requests) = invoke(
        json!({
            "questions": [
                { "kind": "confirm", "prompt": "确认继续？" },
                { "kind": "text_input", "prompt": "任务名称：" }
            ]
        }),
        vec![
            InteractionResponse::Confirmed { allowed: false },
            InteractionResponse::Text {
                value: Some("出题技能".to_string()),
                display_value: None,
            },
        ],
    )
    .await;

    assert_eq!(requests.len(), 2);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["answers"][0]["answer"], "no");
    assert_eq!(parsed["answers"][1]["value"], "出题技能");
}

#[tokio::test]
async fn confirm_timeout_is_reported_as_unanswered_not_no() {
    let prompt = "是否恢复该历史任务，从「需求分析」步骤继续？";
    let (output, requests) = invoke(
        json!({ "questions": [ { "kind": "confirm", "prompt": prompt } ] }),
        vec![InteractionResponse::Unanswered],
    )
    .await;

    assert_eq!(requests.len(), 1);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed,
        json!({
            "answers": [
                { "kind": "confirm", "prompt": prompt, "answer": "unanswered" }
            ]
        })
    );
}

#[tokio::test]
async fn unanswered_response_maps_to_null_value_for_text_and_choice() {
    let (output, _) = invoke(
        json!({
            "questions": [
                { "kind": "text_input", "prompt": "任务名称：" },
                { "kind": "choice", "prompt": "选择处理方式：", "options": ["恢复", "新建"] }
            ]
        }),
        vec![
            InteractionResponse::Unanswered,
            InteractionResponse::Unanswered,
        ],
    )
    .await;

    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed,
        json!({
            "answers": [
                { "kind": "text", "prompt": "任务名称：", "value": null },
                { "kind": "choice", "prompt": "选择处理方式：", "value": null }
            ]
        })
    );
}

#[tokio::test]
async fn mixed_real_and_unanswered_confirm_answers_stay_distinct() {
    let (output, _) = invoke(
        json!({
            "questions": [
                { "kind": "confirm", "prompt": "第一个问题？" },
                { "kind": "confirm", "prompt": "第二个问题？" }
            ]
        }),
        vec![
            InteractionResponse::Confirmed { allowed: false },
            InteractionResponse::Unanswered,
        ],
    )
    .await;

    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["answers"][0]["answer"], "no");
    assert_eq!(parsed["answers"][1]["answer"], "unanswered");
}
