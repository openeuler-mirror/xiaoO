use std::any::Any;

use agent_contracts::hook::hookers::tool_hookers::PreToolHook;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use agent_types::tool::ToolExecutionError;
use agent_types::tool::{PreHookResult, PreToolHookInput};
use async_trait::async_trait;

pub struct BuiltinHelloWorldPreHooker {
    id: HookerId,
    hook_point: HookPointId,
}

impl BuiltinHelloWorldPreHooker {
    pub fn new() -> Self {
        Self {
            id: HookerId("builtin_helloworld_pre_hooker".to_string()),
            hook_point: HookPointId("defaultagent.Tool.builtin-helloworld.pre".to_string()),
        }
    }
}

#[async_trait]
impl Hooker for BuiltinHelloWorldPreHooker {
    fn id(&self) -> &HookerId {
        &self.id
    }

    fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        match input {
            HookInvokeInput::Pre { input, .. } => {
                Ok(HookInvokeOutput::Pre(self.hook(&input, runtime).await))
            }
            HookInvokeInput::Post { .. } => {
                Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "hooker '{}' cannot handle post-hook input for hook point {}",
                        self.id.0, self.hook_point.0
                    ),
                }))
            }
            HookInvokeInput::Error { .. } => {
                Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "hooker '{}' cannot handle error-hook input for hook point {}",
                        self.id.0, self.hook_point.0
                    ),
                }))
            }
            other => Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' cannot handle llm-hook input {:?} for hook point {}",
                    self.id.0, other, self.hook_point.0
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[async_trait]
impl PreToolHook for BuiltinHelloWorldPreHooker {
    async fn hook(&self, _input: &PreToolHookInput, _runtime: &dyn RuntimeView) -> PreHookResult {
        println!("Hook trigered at {}", &self.hook_point.0);
        PreHookResult::Allow
    }
}
