use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hooker::HookPointId;
use agent_types::hooker::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use agent_types::llm::PreLlmHookResult;
use agent_types::tool::ToolExecutionError;
use async_trait::async_trait;

pub struct HelloWorldLlmPreHooker {
    id: HookerId,
    hook_point: HookPointId,
}

impl HelloWorldLlmPreHooker {
    pub fn new() -> Self {
        Self {
            id: HookerId("builtin_helloworld_llm_pre_hooker".to_string()),
            hook_point: HookPointId("defaultagent.Llm.complete.pre".to_string()),
        }
    }
}

#[async_trait]
impl Hooker for HelloWorldLlmPreHooker {
    fn id(&self) -> &HookerId {
        &self.id
    }

    fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        _runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        match input {
            HookInvokeInput::LlmPre {
                input: pre_input, ..
            } => {
                println!(
                    "[HelloWorldLlmPreHooker] hook triggered at '{}', messages={}",
                    self.hook_point.0,
                    pre_input.request.messages.len()
                );
                Ok(HookInvokeOutput::LlmPre(PreLlmHookResult::Allow))
            }
            other => Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' expected LlmPre input but got {:?}",
                    self.id.0, other
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
