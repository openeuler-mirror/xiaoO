use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hooker::HookPointId;
use agent_types::hooker::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use agent_types::llm::PostLlmHookResult;
use agent_types::tool::ToolExecutionError;
use async_trait::async_trait;

pub struct HelloWorldLlmPostHooker {
    id: HookerId,
    hook_point: HookPointId,
}

impl HelloWorldLlmPostHooker {
    pub fn new() -> Self {
        Self {
            id: HookerId("builtin_helloworld_llm_post_hooker".to_string()),
            hook_point: HookPointId("defaultagent.Llm.complete.post".to_string()),
        }
    }
}

#[async_trait]
impl Hooker for HelloWorldLlmPostHooker {
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
            HookInvokeInput::LlmPost(post_input) => {
                let token_count = post_input.response.message.usage.total_tokens;
                println!(
                    "[HelloWorldLlmPostHooker] hook triggered at '{}', total_tokens={}",
                    self.hook_point.0, token_count
                );
                Ok(HookInvokeOutput::LlmPost(PostLlmHookResult::Accept))
            }
            other => Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' expected LlmPost input but got {:?}",
                    self.id.0, other
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
