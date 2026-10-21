use std::any::Any;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::llm::MessageRole;
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput, RawToolOutcome, ToolExecutionError,
};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::super::core::{serialize_workspace_root, PluginHookerCore};
use crate::{resolve_hook_point_category, HookPointCategory};

/// plugin hooker 子进程最长执行时间(10 分钟)。超时后由 `kill_on_drop` 自动兜底杀掉子进程,
/// 防止卡死命令长期阻塞 tokio worker。
const PLUGIN_HOOKER_TIMEOUT_MS: u64 = 600_000;

pub(crate) struct PluginToolHookerAdaptor {
    core: PluginHookerCore,
}

impl PluginToolHookerAdaptor {
    pub fn new(
        id: HookerId,
        hook_point: HookPointId,
        command: String,
        definition: serde_json::Value,
    ) -> Self {
        Self {
            core: PluginHookerCore::new(id, hook_point, command, definition),
        }
    }

    /// Lift a message into the tool-domain plugin error. Passed to
    /// [`PluginHookerCore`] helpers so the shared subprocess/JSON code paths
    /// construct `ToolExecutionError` rather than a foreign error type.
    fn err(message: String) -> ToolExecutionError {
        ToolExecutionError::ExecutionFailed { message }
    }

    /// Tool hook timeouts keep the tool-domain `Timeout` variant (carrying
    /// the cap in milliseconds) instead of a generic message error.
    fn timeout_err(_message: String, timeout_ms: u64) -> ToolExecutionError {
        ToolExecutionError::Timeout { timeout_ms }
    }

    async fn invoke_for_category(
        &self,
        category: HookPointCategory,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        match (category, input) {
            (HookPointCategory::ToolPre, HookInvokeInput::Pre { input, metadata }) => {
                self.invoke_pre(&input, &metadata, runtime).await
            }
            (HookPointCategory::ToolPost, HookInvokeInput::Post { input, metadata }) => {
                self.invoke_post(&input, &metadata, runtime).await
            }
            (HookPointCategory::ToolError, HookInvokeInput::Error { input, metadata }) => {
                self.invoke_error(&input, &metadata, runtime).await
            }
            (category, _) => Err(Self::err(format!(
                "hooker '{}' received mismatched invoke input for category {:?}",
                self.core.id().0,
                category
            ))),
        }
    }

    async fn invoke_pre(
        &self,
        input: &PreToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_pre_payload(input, metadata, runtime)?;
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOKER_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::Pre(self.parse_pre_result(&output)?))
    }

    async fn invoke_post(
        &self,
        input: &PostToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_post_payload(input, metadata, runtime)?;
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOKER_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::Post(self.parse_post_result(&output)?))
    }

    async fn invoke_error(
        &self,
        input: &ErrorToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_error_payload(input, metadata, runtime)?;
        let output = self
            .core
            .resolve_plugin_output(
                payload,
                runtime,
                &Self::err,
                &Self::timeout_err,
                Some(PLUGIN_HOOKER_TIMEOUT_MS),
            )
            .await?;
        Ok(HookInvokeOutput::Error(self.parse_error_result(&output)?))
    }

    /// Session identity emitted in every tool payload: the runtime agent
    /// metadata's session id, falling back to the call id when the runtime
    /// carries none. Shared by the pre/post/error payload builders so all
    /// three stages report the identical session identity for one tool
    /// invocation — post/error hookers can correlate their event with the
    /// session the pre hook saw without cross-process state.
    fn session_identity(runtime: &dyn RuntimeView, call_id: &str) -> String {
        runtime
            .agent_context()
            .metadata()
            .session_id
            .clone()
            .unwrap_or_else(|| call_id.to_string())
    }

    fn build_pre_payload(
        &self,
        input: &PreToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        let session_id = Self::session_identity(runtime, &input.call.call_id);

        let recent_messages = runtime.agent_context().conversation().recent_messages(100);

        // Get the most recent user message as prompt_session (for intent
        // consistency check). The intent check needs the *current* intent,
        // not the first user message of the session: in multi-turn
        // conversations the opening is usually a greeting (e.g. "hi"), and
        // using it would pollute intent detection. Answers to tools like
        // ask_user_question are written back as tool_result (Tool role),
        // never as User messages, so the most recent User message is
        // semantically "the user's latest proactive input" and cannot be
        // polluted by such answers.
        let prompt_session = recent_messages
            .iter()
            .rev()
            .find(|m| m.role == MessageRole::User)
            .and_then(|m| {
                m.blocks.iter().find_map(|b| match b {
                    agent_types::llm::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();

        // Get completed tool call history (for read_before_write rules)
        // Collect ToolUse (with input params like file paths) and ToolResult (with execution results)
        let mut tool_use_map: std::collections::HashMap<&String, Value> =
            std::collections::HashMap::new();

        // First collect all ToolUse, record call_id -> input mapping
        for m in recent_messages.iter() {
            for block in &m.blocks {
                if let agent_types::llm::ContentBlock::ToolUse {
                    call_id,
                    tool_name,
                    input,
                } = block
                {
                    tool_use_map.insert(
                        call_id,
                        json!({
                            "action_type": tool_name,
                            "action_detail": input,
                        }),
                    );
                }
            }
        }

        // Then collect ToolResult, merge input and output
        let action_history: Vec<Value> = recent_messages
            .iter()
            .flat_map(|m| m.blocks.iter())
            .filter_map(|block| match block {
                agent_types::llm::ContentBlock::ToolResult {
                    call_id,
                    tool_name,
                    output,
                    is_error,
                } => {
                    // Merge ToolUse input info
                    let mut entry = tool_use_map.get(call_id).cloned().unwrap_or_else(|| {
                        json!({
                            "action_type": tool_name,
                            "action_detail": "",
                        })
                    });
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("call_id".to_string(), json!(call_id));
                        obj.insert("output".to_string(), json!(output));
                        obj.insert("is_error".to_string(), json!(is_error));
                    }
                    Some(entry)
                }
                _ => None,
            })
            .collect();

        // Build the interleaved history prompt (prompt_history): iterate
        // messages in time order, open a new turn on each User message, and
        // assign subsequent ToolResults to that turn's actions until the
        // next User message. This fixes L2 intent-consistency misses where
        // a "continue" overwrites the initial read/search intent in
        // multi-turn sessions. Matches section 4 of the AgentMoss interface
        // contract (docs/INTEGRATION_GUIDE.md).
        let mut prompt_history: Vec<Value> = Vec::new();
        for m in recent_messages.iter() {
            if m.role == MessageRole::User {
                let text = m
                    .blocks
                    .iter()
                    .find_map(|b| match b {
                        agent_types::llm::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                prompt_history.push(json!({
                    "text": text,
                    "actions": Vec::<Value>::new(),
                }));
            } else {
                // ToolResults in non-User messages belong to the current
                // (most recent) turn's actions
                for block in &m.blocks {
                    if let agent_types::llm::ContentBlock::ToolResult {
                        call_id,
                        tool_name,
                        output,
                        is_error,
                    } = block
                    {
                        if let Some(last) = prompt_history.last_mut() {
                            if let Some(obj) = last.as_object_mut() {
                                let mut action = tool_use_map.get(call_id).cloned().unwrap_or_else(
                                    || json!({"action_type": tool_name, "action_detail": ""}),
                                );
                                if let Some(aobj) = action.as_object_mut() {
                                    aobj.insert("call_id".to_string(), json!(call_id));
                                    aobj.insert("output".to_string(), json!(output));
                                    aobj.insert("is_error".to_string(), json!(is_error));
                                }
                                if let Some(arr) =
                                    obj.get_mut("actions").and_then(|a| a.as_array_mut())
                                {
                                    arr.push(action);
                                }
                            }
                        }
                    }
                }
            }
        }

        let call = serde_json::to_value(&input.call).map_err(|error| {
            Self::err(format!(
                "failed to serialize pre-hook call payload for '{}': {}",
                self.core.id().0,
                error
            ))
        })?;

        Ok(self.core.build_stage_payload(
            "pre",
            metadata,
            runtime,
            vec![
                ("session_id", json!(session_id)),
                ("workspace", serialize_workspace_root(runtime)),
                ("prompt_session", json!(prompt_session)),
                ("prompt_history", json!(prompt_history)),
                ("action_history", json!(action_history)),
                ("call", call),
            ],
        ))
    }

    fn build_post_payload(
        &self,
        input: &PostToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        let session_id = Self::session_identity(runtime, &input.call.call_id);
        let call = serde_json::to_value(&input.call).map_err(|error| {
            Self::err(format!(
                "failed to serialize post-hook call payload for '{}': {}",
                self.core.id().0,
                error
            ))
        })?;

        Ok(self.core.build_stage_payload(
            "post",
            metadata,
            runtime,
            vec![
                ("session_id", json!(session_id)),
                ("workspace", serialize_workspace_root(runtime)),
                ("call", call),
                ("outcome", self.serialize_raw_outcome(&input.outcome)),
            ],
        ))
    }

    fn build_error_payload(
        &self,
        input: &ErrorToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        let session_id = Self::session_identity(runtime, &input.call.call_id);
        let call = serde_json::to_value(&input.call).map_err(|error| {
            Self::err(format!(
                "failed to serialize error-hook call payload for '{}': {}",
                self.core.id().0,
                error
            ))
        })?;

        Ok(self.core.build_stage_payload(
            "error",
            metadata,
            runtime,
            vec![
                ("session_id", json!(session_id)),
                ("workspace", serialize_workspace_root(runtime)),
                ("call", call),
                ("error", self.serialize_execution_error(&input.error)),
            ],
        ))
    }

    fn serialize_raw_outcome(&self, outcome: &RawToolOutcome) -> Value {
        match outcome {
            RawToolOutcome::Success { output } => json!({
                "type": "success",
                "output": output,
            }),
            RawToolOutcome::Error { message } => json!({
                "type": "error",
                "message": message,
            }),
        }
    }

    fn serialize_execution_error(&self, error: &ToolExecutionError) -> Value {
        match error {
            ToolExecutionError::NotFound { tool_name } => json!({
                "type": "not_found",
                "tool_name": tool_name,
                "message": error.to_string(),
            }),
            ToolExecutionError::ExecutionFailed { message } => json!({
                "type": "execution_failed",
                "message": message,
            }),
            ToolExecutionError::Timeout { timeout_ms } => json!({
                "type": "timeout",
                "timeout_ms": timeout_ms,
                "message": error.to_string(),
            }),
            ToolExecutionError::PermissionDenied { message } => json!({
                "type": "permission_denied",
                "message": message,
            }),
        }
    }

    fn parse_pre_result(&self, output: &Value) -> Result<PreHookResult, ToolExecutionError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "allow" => Ok(PreHookResult::Allow {
                extra: self.parse_extra_field(output)?,
            }),
            "deny" => Ok(PreHookResult::Deny {
                reason: self
                    .core
                    .read_required_string_field(output, "reason", Self::err)?
                    .to_string(),
            }),
            "transform" => Ok(PreHookResult::Transform {
                modified_input: self
                    .core
                    .read_required_value_field(output, "modified_input", Self::err)?
                    .clone(),
                extra: self.parse_extra_field(output)?,
            }),
            result => Err(Self::err(format!(
                "plugin tool pre-hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    /// Parse the optional `extra` field of a pre-hook response: a missing key
    /// or an explicit `null` both mean "no contribution" (normalized to
    /// `None`); any other value must be a JSON object so the framework can
    /// attribute the entry via the injected `hooker_id` key.
    fn parse_extra_field(&self, output: &Value) -> Result<Option<Value>, ToolExecutionError> {
        match output.get("extra") {
            None | Some(Value::Null) => Ok(None),
            Some(value) if value.is_object() => Ok(Some(value.clone())),
            Some(value) => Err(Self::err(format!(
                "plugin tool pre-hooker '{}' returned a non-object 'extra' payload ({}), \
                 expected a JSON object or null",
                self.core.id().0,
                Self::json_kind(value),
            ))),
        }
    }

    /// Short JSON type tag used in `extra` validation errors.
    fn json_kind(value: &Value) -> &'static str {
        match value {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    fn parse_post_result(&self, output: &Value) -> Result<PostHookResult, ToolExecutionError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "accept" => Ok(PostHookResult::Accept),
            "transform" => Ok(PostHookResult::Transform {
                modified_output: self
                    .core
                    .read_required_string_field(output, "modified_output", Self::err)?
                    .to_string(),
            }),
            result => Err(Self::err(format!(
                "plugin tool post-hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }

    fn parse_error_result(&self, output: &Value) -> Result<ErrorHookResult, ToolExecutionError> {
        match self
            .core
            .read_required_result_tag(output, Self::err)?
            .as_str()
        {
            "propagate" => Ok(ErrorHookResult::Propagate),
            "recover" => Ok(ErrorHookResult::Recover {
                output: self
                    .core
                    .read_required_string_field(output, "output", Self::err)?
                    .to_string(),
            }),
            result => Err(Self::err(format!(
                "plugin tool error-hooker '{}' returned unsupported result '{}'",
                self.core.id().0,
                result
            ))),
        }
    }
}

#[async_trait]
impl Hooker for PluginToolHookerAdaptor {
    fn id(&self) -> &HookerId {
        self.core.id()
    }

    fn hook_point(&self) -> &HookPointId {
        self.core.hook_point()
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let category = resolve_hook_point_category(self.core.hook_point()).map_err(|error| {
            HookInvokeError::Tool(Self::err(format!(
                "failed to resolve hook point category for hooker '{}': {}",
                self.core.id().0,
                error
            )))
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
#[path = "../../../../../../tests/unit/hook/hookers/plugin/tool/adaptor_test.rs"]
mod tests;
