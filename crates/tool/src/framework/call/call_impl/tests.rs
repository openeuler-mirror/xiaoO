use super::state::ToolExecutionState;
use super::ToolCallImpl;
use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use agent_types::tool::{
    FinalToolCall, PreHookResult, ToolExecutionError, ToolExecutionResult, ToolExecutorOutput,
};
use async_trait::async_trait;
use std::sync::Arc;

struct TestToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
}

impl ToolSpecView for TestToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        static DEFAULT: OutputContract = OutputContract {
            description: String::new(),
        };
        &DEFAULT
    }

    fn effect_profile(&self) -> &EffectProfile {
        static DEFAULT: EffectProfile = EffectProfile {
            reads_filesystem: false,
            writes_filesystem: false,
            network_access: false,
            side_effects: false,
        };
        &DEFAULT
    }
}

struct TestToolExecutor {
    spec: Arc<TestToolSpec>,
}

#[async_trait]
impl ToolExecutor for TestToolExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        _call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        panic!("unused in call_impl unit tests")
    }
}

fn test_tool_call_impl() -> ToolCallImpl {
    let spec = Arc::new(TestToolSpec {
        id: ToolId("test_tool".to_string()),
        name: ToolName("test_tool".to_string()),
        description: "test tool".to_string(),
        input_schema: InputSchemaRef {
            schema: serde_json::json!({"type": "object"}),
        },
    });
    let executor: Arc<dyn ToolExecutor> = Arc::new(TestToolExecutor { spec: spec.clone() });
    let spec: Arc<dyn ToolSpecView> = spec;

    ToolCallImpl::new(
        FinalToolCall {
            call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::json!({"value": 1}),
        },
        spec,
        executor,
    )
}

fn test_execution_state() -> ToolExecutionState {
    ToolExecutionState {
        final_call: FinalToolCall {
            call_id: "call-1".to_string(),
            tool_name: "test_tool".to_string(),
            input: serde_json::json!({"value": 1}),
        },
        trace_span: None,
        lifecycle_record: None,
        pre_hook_results: Vec::new(),
        post_hook_results: Vec::new(),
        error_hook_results: Vec::new(),
        raw_outcome: None,
        execution_error: None,
    }
}

#[test]
fn build_denied_result_uses_pre_hook_reason_when_error_missing() {
    let call_impl = test_tool_call_impl();
    let mut state = test_execution_state();
    state.pre_hook_results = vec![PreHookResult::Deny {
        reason: "blocked by policy".to_string(),
    }];

    let result = call_impl.build_denied_result(&state);

    match result {
        ToolExecutionResult::Denied { error, .. } => {
            assert!(matches!(
                error,
                Some(ToolExecutionError::PermissionDenied { message }) if message == "blocked by policy"
            ));
        }
        other => panic!("expected denied result, got {:?}", other),
    }
}

#[test]
fn build_denied_result_preserves_existing_execution_error() {
    let call_impl = test_tool_call_impl();
    let mut state = test_execution_state();
    state.pre_hook_results = vec![PreHookResult::Deny {
        reason: "blocked by policy".to_string(),
    }];
    state.execution_error = Some(ToolExecutionError::ExecutionFailed {
        message: "existing error".to_string(),
    });

    let result = call_impl.build_denied_result(&state);

    match result {
        ToolExecutionResult::Denied { error, .. } => {
            assert!(matches!(
                error,
                Some(ToolExecutionError::ExecutionFailed { message }) if message == "existing error"
            ));
        }
        other => panic!("expected denied result, got {:?}", other),
    }
}
