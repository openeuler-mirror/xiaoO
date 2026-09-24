use std::future::Future;
use std::sync::Mutex;

use agent_contracts::{
    AgentContext, HookerRegistry, InteractionHandle, ToolEventSink, ToolStateStore, TraceRecorder,
};

use super::*;

/// tokio "macros" is not enabled for the tool crate.
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
