use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::chat::{
    ChatHookError, ChatMessageHookInput, ChatMessageHookResult, ChatSystemTransformInput,
    ChatSystemTransformResult, CommandExecuteBeforeInput, CommandExecuteBeforeResult,
};
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::llm::ChatMessage;
use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::core::{serialize_workspace_root, PluginHookerCore};
use super::super::PLUGIN_HOOK_COMMAND_TIMEOUT_MS;
use crate::{resolve_hook_point_category, HookPointCategory};

pub(crate) struct PluginChatHookerAdaptor {
    core: PluginHookerCore,
}

impl PluginChatHookerAdaptor {
    pub fn new(
        id: agent_types::common::HookerId,
        hook_point: agent_types::hook::HookPointId,
        command: String,
        definition: Value,
    ) -> Self {
        Self {
            core: PluginHookerCore::new(id, hook_point, command, definition),
        }
    }

    /// Lift a message into the chat-domain plugin error. Passed to
    /// [`PluginHookerCore`] helpers so the shared subprocess/JSON code paths
    /// construct `ChatHookError` rather than a foreign error type.
    fn err(message: String) -> ChatHookError {
        ChatHookError::Plugin { message }
    }

    /// Chat hook timeouts surface as the same plugin error, carrying the
    /// driver's preformatted timeout message.
    fn timeout_err(message: String, _timeout_ms: u64) -> ChatHookError {
        Self::err(message)
    }

    async fn invoke_for_category(
        &self,
        category: HookPointCategory,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ChatHookError> {
        match (category, input) {
            (
                HookPointCategory::ChatSystemTransform,
                HookInvokeInput::ChatSystemTransform { input, metadata },
            ) => {
                self.invoke_system_transform(&input, &metadata, runtime)
                    .await
            }
            (HookPointCategory::ChatMessage, HookInvokeInput::ChatMessage { input, metadata }) => {
                self.invoke_chat_message(&input, &metadata, runtime).await
            }
            (
                HookPointCategory::CommandExecuteBefore,
                HookInvokeInput::CommandExecuteBefore { input, metadata },
            ) => self.invoke_command_before(&input, &metadata, runtime).await,
            (category, _) => Err(Self::err(format!(
                "chat hooker '{}' received mismatched invoke input for category {:?}",
                self.core.id().0,
                category
            ))),
        }
    }

    async fn invoke_system_transform(
        &self,
        input: &ChatSystemTransformInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ChatHookError> {
        let payload = self.build_system_transform_payload(input, metadata, runtime);
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOK_COMMAND_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::ChatSystemTransform(
            self.parse_system_transform_result(&output)?,
        ))
    }

    async fn invoke_chat_message(
        &self,
        input: &ChatMessageHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ChatHookError> {
        let payload = self.build_chat_message_payload(input, metadata, runtime);
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOK_COMMAND_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::ChatMessage(
            self.parse_chat_message_result(&output)?,
        ))
    }

    async fn invoke_command_before(
        &self,
        input: &CommandExecuteBeforeInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ChatHookError> {
        let payload = self.build_command_before_payload(input, metadata, runtime);
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOK_COMMAND_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::CommandExecuteBefore(
            self.parse_command_before_result(&output)?,
        ))
    }

    fn build_system_transform_payload(
        &self,
        input: &ChatSystemTransformInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        self.core.build_stage_payload(
            "system_transform",
            metadata,
            runtime,
            vec![
                ("session_id", json!(input.session_id)),
                ("workspace", serialize_workspace_root(runtime)),
                (
                    "model",
                    json!({
                        "provider_id": input.model.provider_id,
                        "model_id": input.model.model_id,
                    }),
                ),
                ("system", json!(input.current_system)),
            ],
        )
    }

    fn build_chat_message_payload(
        &self,
        input: &ChatMessageHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        self.core.build_stage_payload(
            "chat_message",
            metadata,
            runtime,
            vec![
                ("session_id", json!(input.session_id)),
                ("workspace", serialize_workspace_root(runtime)),
                ("agent", json!(input.agent)),
                (
                    "model",
                    json!(input.model.as_ref().map(|m| json!({
                        "provider_id": m.provider_id,
                        "model_id": m.model_id,
                    }))),
                ),
                ("message_id", json!(input.message_id)),
                ("message", json!(input.message)),
                ("prior_message_count", json!(input.prior_message_count)),
            ],
        )
    }

    fn build_command_before_payload(
        &self,
        input: &CommandExecuteBeforeInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        self.core.build_stage_payload(
            "command_before",
            metadata,
            runtime,
            vec![
                ("command", json!(input.command)),
                ("session_id", json!(input.session_id)),
                ("workspace", serialize_workspace_root(runtime)),
                ("arguments", json!(input.arguments)),
                ("body", json!(input.body)),
            ],
        )
    }

    fn parse_system_transform_result(
        &self,
        output: &Value,
    ) -> Result<ChatSystemTransformResult, ChatHookError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "allow" => Ok(ChatSystemTransformResult::Allow),
            "transform" => {
                let system = self
                    .core
                    .read_required_value_field(output, "system", Self::err)?;
                let system: Vec<String> =
                    serde_json::from_value(system.clone()).map_err(|error| {
                        Self::err(format!(
                            "plugin chat hooker '{}' returned invalid system array: {}",
                            self.core.id().0,
                            error
                        ))
                    })?;
                Ok(ChatSystemTransformResult::Transform { system })
            }
            result => Err(Self::err(format!(
                "plugin chat hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    fn parse_chat_message_result(
        &self,
        output: &Value,
    ) -> Result<ChatMessageHookResult, ChatHookError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "accept" => Ok(ChatMessageHookResult::Accept),
            "transform" => {
                let message_value =
                    self.core
                        .read_required_value_field(output, "message", Self::err)?;
                let message: ChatMessage =
                    serde_json::from_value(message_value.clone()).map_err(|error| {
                        Self::err(format!(
                            "plugin chat hooker '{}' returned invalid message: {}",
                            self.core.id().0,
                            error
                        ))
                    })?;
                Ok(ChatMessageHookResult::Transform { message })
            }
            result => Err(Self::err(format!(
                "plugin chat hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    fn parse_command_before_result(
        &self,
        output: &Value,
    ) -> Result<CommandExecuteBeforeResult, ChatHookError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "allow" => Ok(CommandExecuteBeforeResult::Allow),
            "transform" => {
                let body = self
                    .core
                    .read_required_string_field(output, "body", Self::err)?
                    .to_string();
                Ok(CommandExecuteBeforeResult::Transform { body })
            }
            "deny" => {
                let reason = output
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("denied by plugin")
                    .to_string();
                Ok(CommandExecuteBeforeResult::Deny { reason })
            }
            result => Err(Self::err(format!(
                "plugin chat hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }
}

#[async_trait]
impl Hooker for PluginChatHookerAdaptor {
    fn id(&self) -> &agent_types::common::HookerId {
        self.core.id()
    }

    fn hook_point(&self) -> &agent_types::hook::HookPointId {
        self.core.hook_point()
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let category = resolve_hook_point_category(self.core.hook_point()).map_err(|error| {
            Self::err(format!(
                "failed to resolve hook point category for hooker '{}': {}",
                self.core.id().0,
                error
            ))
        })?;

        self.invoke_for_category(category, input, runtime)
            .await
            .map_err(HookInvokeError::from)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/hook/hookers/plugin/chat/adaptor_test.rs"]
mod tests;
