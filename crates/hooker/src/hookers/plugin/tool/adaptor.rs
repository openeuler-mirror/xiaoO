use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hooker::HookPointId;
use agent_types::hooker::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use agent_types::tool::ToolExecutionError;
use async_trait::async_trait;

use crate::{resolve_hook_point_category, HookPointCategory};

pub struct PluginToolHookerAdaptor {
    id: HookerId,
    hook_point: HookPointId,
    command: String,
    definition: serde_json::Value,
}

impl PluginToolHookerAdaptor {
    pub fn new(
        id: HookerId,
        hook_point: HookPointId,
        command: String,
        definition: serde_json::Value,
    ) -> Self {
        Self {
            id,
            hook_point,
            command,
            definition,
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn definition(&self) -> &serde_json::Value {
        &self.definition
    }
}

#[async_trait]
impl Hooker for PluginToolHookerAdaptor {
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
        let category = resolve_hook_point_category(&self.hook_point).map_err(|error| {
            HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to resolve hook point category for hooker '{}': {}",
                    self.id.0, error
                ),
            })
        })?;

        match (category, input) {
            (HookPointCategory::ToolPre, HookInvokeInput::Pre(_))
            | (HookPointCategory::ToolPost, HookInvokeInput::Post(_))
            | (HookPointCategory::ToolError, HookInvokeInput::Error(_)) => {
                Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "plugin tool hooker '{}' command '{}' invoke path is not implemented yet",
                        self.id.0, self.command
                    ),
                }))
            }
            (category, _) => Err(HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' received mismatched invoke input for category {:?}",
                    self.id.0, category
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
