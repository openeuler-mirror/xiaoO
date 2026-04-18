use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hooker::HookPointId;
use agent_types::hooker::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use agent_types::session::SessionHookResult;
use agent_types::tool::ToolExecutionError;
use async_trait::async_trait;

pub struct BuiltinSessionCreatedHooker {
    id: HookerId,
    hook_point: HookPointId,
}

impl BuiltinSessionCreatedHooker {
    pub fn new() -> Self {
        Self {
            id: HookerId("builtin_session_created_hooker".to_string()),
            hook_point: HookPointId("defaultagent.Session.lifecycle.created".to_string()),
        }
    }
}

#[async_trait]
impl Hooker for BuiltinSessionCreatedHooker {
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
            HookInvokeInput::SessionCreated { input, .. } => {
                println!(
                    "[BuiltinSessionCreatedHooker] session '{}' created for sender '{}'",
                    input.session_id, input.sender_id
                );
                Ok(HookInvokeOutput::SessionCreated(SessionHookResult::Acknowledged))
            }
            other => Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' expected SessionCreated input but got {:?}",
                    self.id.0, other
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
