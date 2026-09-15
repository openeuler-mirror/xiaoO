use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::chat::{
    ChatHookError, ChatMessageHookInput, ChatMessageHookResult, ChatSystemTransformInput,
    ChatSystemTransformResult, CommandExecuteBeforeInput, CommandExecuteBeforeResult,
};
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::interaction::types::InteractionSource;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::llm::ChatMessage;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::core::PluginHookerCore;
use super::super::PLUGIN_HOOK_COMMAND_TIMEOUT_MS;
use crate::{resolve_hook_point_category, HookPointCategory};

pub(crate) struct PluginChatHookerAdaptor {
    core: PluginHookerCore,
}

#[derive(Debug)]
enum PluginCommandResponse {
    Final(Value),
    AskUser(AskUserDirective),
}

#[derive(Debug)]
struct AskUserDirective {
    request: PluginAskUserRequest,
    continuation: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PluginAskUserRequest {
    Confirm {
        prompt: String,
    },
    TextInput {
        prompt: String,
    },
    Choice {
        prompt: String,
        options: Vec<String>,
        allow_custom_input: bool,
    },
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
        let output = self.resolve_plugin_output(payload, runtime).await?;
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
        let output = self.resolve_plugin_output(payload, runtime).await?;
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
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::CommandExecuteBefore(
            self.parse_command_before_result(&output)?,
        ))
    }

    async fn resolve_plugin_output(
        &self,
        initial_payload: Value,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ChatHookError> {
        let mut payload = initial_payload;

        loop {
            let output = self
                .core
                .run_plugin_command(&payload, Self::err, Some(PLUGIN_HOOK_COMMAND_TIMEOUT_MS))
                .await?;
            match self.parse_plugin_command_response(output)? {
                PluginCommandResponse::Final(final_output) => return Ok(final_output),
                PluginCommandResponse::AskUser(directive) => {
                    let request = self.with_hooker_interaction_source(directive.request);
                    let response = runtime.interaction().ask(&request).await;
                    payload = self.build_interaction_followup_payload(
                        payload,
                        directive.continuation,
                        &request,
                        &response,
                    )?;
                }
            }
        }
    }

    fn build_system_transform_payload(
        &self,
        input: &ChatSystemTransformInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        json!({
            "stage": "system_transform",
            "hooker": self.core.serialize_hooker_info(runtime),
            "metadata": self.core.serialize_metadata(metadata),
            "session_id": input.session_id,
            "model": {
                "provider_id": input.model.provider_id,
                "model_id": input.model.model_id,
            },
            "system": input.current_system,
            "policy": runtime.hookers().policy_for(self.core.id()).cloned(),
            "definition": self.core.definition().clone(),
        })
    }

    fn build_chat_message_payload(
        &self,
        input: &ChatMessageHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        json!({
            "stage": "chat_message",
            "hooker": self.core.serialize_hooker_info(runtime),
            "metadata": self.core.serialize_metadata(metadata),
            "session_id": input.session_id,
            "agent": input.agent,
            "model": input.model.as_ref().map(|m| json!({
                "provider_id": m.provider_id,
                "model_id": m.model_id,
            })),
            "message_id": input.message_id,
            "message": input.message,
            "prior_message_count": input.prior_message_count,
            "policy": runtime.hookers().policy_for(self.core.id()).cloned(),
            "definition": self.core.definition().clone(),
        })
    }

    fn build_command_before_payload(
        &self,
        input: &CommandExecuteBeforeInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Value {
        json!({
            "stage": "command_before",
            "hooker": self.core.serialize_hooker_info(runtime),
            "metadata": self.core.serialize_metadata(metadata),
            "command": input.command,
            "session_id": input.session_id,
            "arguments": input.arguments,
            "body": input.body,
            "policy": runtime.hookers().policy_for(self.core.id()).cloned(),
            "definition": self.core.definition().clone(),
        })
    }

    fn parse_plugin_command_response(
        &self,
        output: Value,
    ) -> Result<PluginCommandResponse, ChatHookError> {
        match output.get("action").and_then(Value::as_str) {
            None | Some("final") => Ok(PluginCommandResponse::Final(output)),
            Some("ask_user") => {
                let request = serde_json::from_value(
                    self.read_required_value_field(&output, "request")
                        .cloned()?,
                )
                .map_err(|error| {
                    Self::err(format!(
                        "plugin hooker '{}' ask_user request is invalid: {}",
                        self.core.id().0,
                        error
                    ))
                })?;
                let continuation = self
                    .read_required_value_field(&output, "continuation")
                    .cloned()?;
                Ok(PluginCommandResponse::AskUser(AskUserDirective {
                    request,
                    continuation,
                }))
            }
            Some(other) => Err(Self::err(format!(
                "plugin hooker '{}' returned unsupported action '{}'",
                self.core.id().0,
                other
            ))),
        }
    }

    fn with_hooker_interaction_source(&self, request: PluginAskUserRequest) -> InteractionRequest {
        let source = Some(InteractionSource::Hooker {
            hooker_name: self.core.id().0.clone(),
            hook_point: self.core.hook_point().0.clone(),
        });

        match request {
            PluginAskUserRequest::Confirm { prompt } => {
                InteractionRequest::Confirm { prompt, source }
            }
            PluginAskUserRequest::TextInput { prompt } => InteractionRequest::TextInput {
                prompt,
                source,
                is_secret: false,
            },
            PluginAskUserRequest::Choice {
                prompt,
                options,
                allow_custom_input,
            } => InteractionRequest::Choice {
                prompt,
                options,
                allow_custom_input,
                source,
            },
        }
    }

    fn build_interaction_followup_payload(
        &self,
        payload: Value,
        continuation: Value,
        request: &InteractionRequest,
        response: &InteractionResponse,
    ) -> Result<Value, ChatHookError> {
        let mut payload_map = match payload {
            Value::Object(map) => map,
            _ => {
                return Err(Self::err(format!(
                    "plugin hooker '{}' follow-up payload must be a JSON object",
                    self.core.id().0
                )));
            }
        };

        payload_map.insert(
            "interaction".to_string(),
            json!({
                "request": request,
                "response": response,
                "continuation": continuation,
            }),
        );
        Ok(Value::Object(payload_map))
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
                let system = self.read_required_value_field(output, "system")?;
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
                let message_value = self.read_required_value_field(output, "message")?;
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

    fn read_required_value_field<'a>(
        &self,
        output: &'a Value,
        field_name: &str,
    ) -> Result<&'a Value, ChatHookError> {
        output.get(field_name).ok_or_else(|| {
            Self::err(format!(
                "plugin hooker '{}' response must contain field '{}'",
                self.core.id().0,
                field_name
            ))
        })
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
