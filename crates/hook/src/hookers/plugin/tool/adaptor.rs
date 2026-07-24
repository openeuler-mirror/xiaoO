use std::any::Any;
use std::process::Stdio;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::Hooker;
use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::interaction::types::InteractionSource;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::llm::MessageRole;
use agent_types::tool::{
    ErrorHookResult, ErrorToolHookInput, PostHookResult, PostToolHookInput, PreHookResult,
    PreToolHookInput, RawToolOutcome, ToolExecutionError,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use crate::{resolve_hook_point_category, HookPointCategory};

/// plugin hooker 子进程最长执行时间(10 分钟)。超时后由 `kill_on_drop` 自动兜底杀掉子进程,
/// 防止卡死命令长期阻塞 tokio worker。
const PLUGIN_HOOKER_TIMEOUT: Duration = Duration::from_secs(600);

pub(crate) struct PluginToolHookerAdaptor {
    id: HookerId,
    hook_point: HookPointId,
    command: String,
    definition: serde_json::Value,
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
            (category, _) => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "hooker '{}' received mismatched invoke input for category {:?}",
                    self.id.0, category
                ),
            }),
        }
    }

    async fn invoke_pre(
        &self,
        input: &PreToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_pre_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::Pre(self.parse_pre_result(&output)?))
    }

    async fn invoke_post(
        &self,
        input: &PostToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_post_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::Post(self.parse_post_result(&output)?))
    }

    async fn invoke_error(
        &self,
        input: &ErrorToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, ToolExecutionError> {
        let payload = self.build_error_payload(input, metadata, runtime)?;
        let output = self.resolve_plugin_output(payload, runtime).await?;
        Ok(HookInvokeOutput::Error(self.parse_error_result(&output)?))
    }

    async fn resolve_plugin_output(
        &self,
        initial_payload: Value,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        let mut payload = initial_payload;

        loop {
            let output = self.run_plugin_command(&payload).await?;
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

    fn build_pre_payload(
        &self,
        input: &PreToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        // Get session_id (for cache key)
        let session_id = runtime
            .agent_context()
            .metadata()
            .session_id
            .clone()
            .unwrap_or_else(|| input.call.call_id.clone());

        let recent_messages = runtime.agent_context().conversation().recent_messages(100);

        // Get the first user message as prompt_session (for intent consistency check)
        let prompt_session = recent_messages
            .iter()
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
        let messages = recent_messages;
        let mut tool_use_map: std::collections::HashMap<&String, Value> =
            std::collections::HashMap::new();

        // First collect all ToolUse, record call_id -> input mapping
        for m in messages.iter() {
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
        let action_history: Vec<Value> = messages
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

        Ok(json!({
            "stage": "pre",
            "session_id": session_id,
            "prompt_session": prompt_session,
            "action_history": action_history,
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "call": serde_json::to_value(&input.call).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize pre-hook call payload for '{}': {}", self.id.0, error),
            })?,
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn build_post_payload(
        &self,
        input: &PostToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "post",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "call": serde_json::to_value(&input.call).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize post-hook call payload for '{}': {}", self.id.0, error),
            })?,
            "outcome": self.serialize_raw_outcome(&input.outcome),
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn build_error_payload(
        &self,
        input: &ErrorToolHookInput,
        metadata: &HookInvokeMetadata,
        runtime: &dyn RuntimeView,
    ) -> Result<Value, ToolExecutionError> {
        Ok(json!({
            "stage": "error",
            "hooker": self.serialize_hooker_info(runtime),
            "metadata": self.serialize_metadata(metadata),
            "call": serde_json::to_value(&input.call).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!("failed to serialize error-hook call payload for '{}': {}", self.id.0, error),
            })?,
            "error": self.serialize_execution_error(&input.error),
            "policy": runtime.hookers().policy_for(self.id()).cloned(),
            "definition": self.definition.clone(),
        }))
    }

    fn serialize_hooker_info(&self, runtime: &dyn RuntimeView) -> Value {
        json!({
            "id": self.id.0,
            "hook_point": self.hook_point.0,
            "command": self.command,
            "agent_id": runtime.agent_context().metadata().agent_id,
        })
    }

    fn serialize_metadata(&self, metadata: &HookInvokeMetadata) -> Value {
        json!({
            "trace_id": metadata.trace_id,
            "span_id": metadata.span_id,
            "parent_span_id": metadata.parent_span_id,
        })
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

    async fn run_plugin_command(&self, payload: &Value) -> Result<Value, ToolExecutionError> {
        self.run_plugin_command_with_timeout(payload, PLUGIN_HOOKER_TIMEOUT)
            .await
    }

    async fn run_plugin_command_with_timeout(
        &self,
        payload: &Value,
        cmd_timeout: Duration,
    ) -> Result<Value, ToolExecutionError> {
        let payload_bytes =
            serde_json::to_vec(payload).map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to serialize plugin command payload for hooker '{}': {}",
                    self.id.0, error
                ),
            })?;

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to spawn plugin command for hooker '{}' (command='{}'): {}",
                    self.id.0, self.command, error
                ),
            })?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&payload_bytes).await.map_err(|error| {
                ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "failed to write stdin for plugin hooker '{}' (command='{}'): {}",
                        self.id.0, self.command, error
                    ),
                }
            })?;
        }

        let output = timeout(cmd_timeout, child.wait_with_output())
            .await
            .map_err(|_| ToolExecutionError::Timeout {
                timeout_ms: cmd_timeout.as_millis() as u64,
            })?
            .map_err(|error| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to wait for plugin hooker '{}' (command='{}'): {}",
                    self.id.0, self.command, error
                ),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' command '{}' exited with status {}{}",
                    self.id.0,
                    self.command,
                    output.status,
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", stderr)
                    }
                ),
            });
        }

        serde_json::from_slice(&output.stdout).map_err(|error| {
            ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' command '{}' returned invalid JSON: {}",
                    self.id.0, self.command, error
                ),
            }
        })
    }

    fn parse_plugin_command_response(
        &self,
        output: Value,
    ) -> Result<PluginCommandResponse, ToolExecutionError> {
        match output.get("action").and_then(Value::as_str) {
            None | Some("final") => Ok(PluginCommandResponse::Final(output)),
            Some("ask_user") => {
                let request = serde_json::from_value(
                    self.read_required_value_field(&output, "request")?.clone(),
                )
                .map_err(|error| ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "plugin hooker '{}' ask_user request is invalid: {}",
                        self.id.0, error
                    ),
                })?;
                let continuation = self
                    .read_required_value_field(&output, "continuation")?
                    .clone();
                Ok(PluginCommandResponse::AskUser(AskUserDirective {
                    request,
                    continuation,
                }))
            }
            Some(other) => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' returned unsupported action '{}'",
                    self.id.0, other
                ),
            }),
        }
    }

    fn with_hooker_interaction_source(&self, request: PluginAskUserRequest) -> InteractionRequest {
        let source = Some(InteractionSource::Hooker {
            hooker_name: self.id.0.clone(),
            hook_point: self.hook_point.0.clone(),
        });

        match request {
            PluginAskUserRequest::Confirm { prompt } => {
                InteractionRequest::Confirm { prompt, source }
            }
            PluginAskUserRequest::TextInput { prompt } => {
                InteractionRequest::TextInput {
                    prompt,
                    source,
                    is_secret: false, // Default to false for plugin requests
                }
            }
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
    ) -> Result<Value, ToolExecutionError> {
        let mut payload_map = match payload {
            Value::Object(map) => map,
            _ => {
                return Err(ToolExecutionError::ExecutionFailed {
                    message: format!(
                        "plugin hooker '{}' follow-up payload must be a JSON object",
                        self.id.0
                    ),
                });
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

    fn parse_pre_result(&self, output: &Value) -> Result<PreHookResult, ToolExecutionError> {
        match self.read_required_result_tag(output)?.as_str() {
            "allow" => Ok(PreHookResult::Allow),
            "deny" => Ok(PreHookResult::Deny {
                reason: self
                    .read_required_string_field(output, "reason")?
                    .to_string(),
            }),
            "transform" => Ok(PreHookResult::Transform {
                modified_input: self
                    .read_required_value_field(output, "modified_input")?
                    .clone(),
            }),
            result => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin tool pre-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_post_result(&self, output: &Value) -> Result<PostHookResult, ToolExecutionError> {
        match self.read_required_result_tag(output)?.as_str() {
            "accept" => Ok(PostHookResult::Accept),
            "transform" => Ok(PostHookResult::Transform {
                modified_output: self
                    .read_required_string_field(output, "modified_output")?
                    .to_string(),
            }),
            result => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin tool post-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn parse_error_result(&self, output: &Value) -> Result<ErrorHookResult, ToolExecutionError> {
        match self.read_required_result_tag(output)?.as_str() {
            "propagate" => Ok(ErrorHookResult::Propagate),
            "recover" => Ok(ErrorHookResult::Recover {
                output: self
                    .read_required_string_field(output, "output")?
                    .to_string(),
            }),
            result => Err(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin tool error-hooker '{}' returned unsupported result '{}'",
                    self.id.0, result
                ),
            }),
        }
    }

    fn read_required_result_tag(&self, output: &Value) -> Result<String, ToolExecutionError> {
        Ok(self
            .read_required_string_field(output, "result")?
            .to_lowercase())
    }

    fn read_required_string_field<'a>(
        &self,
        output: &'a Value,
        field_name: &str,
    ) -> Result<&'a str, ToolExecutionError> {
        output
            .get(field_name)
            .and_then(Value::as_str)
            .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' response must contain string field '{}'",
                    self.id.0, field_name
                ),
            })
    }

    fn read_required_value_field<'a>(
        &self,
        output: &'a Value,
        field_name: &str,
    ) -> Result<&'a Value, ToolExecutionError> {
        output
            .get(field_name)
            .ok_or_else(|| ToolExecutionError::ExecutionFailed {
                message: format!(
                    "plugin hooker '{}' response must contain field '{}'",
                    self.id.0, field_name
                ),
            })
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
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let category = resolve_hook_point_category(&self.hook_point).map_err(|error| {
            HookInvokeError::Tool(ToolExecutionError::ExecutionFailed {
                message: format!(
                    "failed to resolve hook point category for hooker '{}': {}",
                    self.id.0, error
                ),
            })
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
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use agent_contracts::events::tool_events::ToolEventSink;
    use agent_contracts::hook::registry::HookerRegistry;
    use agent_contracts::interaction::handle::InteractionHandle;
    use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
    use agent_contracts::runtime::runtime_view::RuntimeView;
    use agent_contracts::tool::state::ToolStateStore;
    use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
    use agent_types::common::{AgentMetadata, WorkspaceRef};
    use agent_types::events::ToolLifecycleEvent;
    use agent_types::hook::HookInvokePrimary;
    use agent_types::tool::FinalToolCall;
    use agent_types::ChatMessage;

    struct TestConversation;

    impl ConversationView for TestConversation {
        fn recent_messages(&self, _limit: usize) -> Vec<ChatMessage> {
            Vec::new()
        }

        fn message_count(&self) -> usize {
            0
        }
    }

    struct TestAgentContext {
        conversation: TestConversation,
        workspace: WorkspaceRef,
        metadata: AgentMetadata,
    }

    impl TestAgentContext {
        fn new() -> Self {
            Self {
                conversation: TestConversation,
                workspace: WorkspaceRef {
                    root: PathBuf::from("/tmp"),
                },
                metadata: AgentMetadata {
                    agent_id: "test-agent".to_string(),
                    model: "test-model".to_string(),
                    session_id: Some("session-1".to_string()),
                },
            }
        }
    }

    impl AgentContext for TestAgentContext {
        fn conversation(&self) -> &dyn ConversationView {
            &self.conversation
        }

        fn workspace(&self) -> &WorkspaceRef {
            &self.workspace
        }

        fn metadata(&self) -> &AgentMetadata {
            &self.metadata
        }
    }

    struct TestInteractionHandle {
        response: InteractionResponse,
        requests: Mutex<Vec<InteractionRequest>>,
    }

    impl TestInteractionHandle {
        fn new(response: InteractionResponse) -> Self {
            Self {
                response,
                requests: Mutex::new(Vec::new()),
            }
        }

        fn recorded_requests(&self) -> Vec<InteractionRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl InteractionHandle for TestInteractionHandle {
        async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
            self.requests.lock().unwrap().push(request.clone());
            self.response.clone()
        }
    }

    struct TestHookerRegistry;

    impl HookerRegistry for TestHookerRegistry {
        fn get(&self, _id: &HookerId) -> Option<&dyn Hooker> {
            None
        }

        fn list(&self) -> Vec<&dyn Hooker> {
            Vec::new()
        }

        fn list_for_hook_point(&self, _hook_point: &HookPointId) -> Vec<&dyn Hooker> {
            Vec::new()
        }

        fn is_enabled(&self, _id: &HookerId) -> bool {
            false
        }

        fn policy_for(&self, _id: &HookerId) -> Option<&serde_json::Value> {
            None
        }
    }

    struct TestToolStateStore;

    impl ToolStateStore for TestToolStateStore {
        fn begin(
            &self,
            _call: &FinalToolCall,
            _spec: &dyn agent_contracts::tool::spec::ToolSpecView,
        ) -> agent_types::tool::ToolLifecycleRecord {
            panic!("unused in adaptor test")
        }

        fn update(&self, _record: &agent_types::tool::ToolLifecycleRecord) {
            panic!("unused in adaptor test")
        }

        fn finish(
            &self,
            _record: &agent_types::tool::ToolLifecycleRecord,
            _result: &agent_types::tool::execution_types::ToolExecutionResult,
        ) {
            panic!("unused in adaptor test")
        }

        fn fail(
            &self,
            _record: &agent_types::tool::ToolLifecycleRecord,
            _error: &ToolExecutionError,
        ) {
            panic!("unused in adaptor test")
        }
    }

    struct TestToolEventSink;

    impl ToolEventSink for TestToolEventSink {
        fn emit(&self, _event: ToolLifecycleEvent) {}
    }

    struct TestTraceRecorder;

    #[async_trait]
    impl TraceRecorder for TestTraceRecorder {
        async fn begin_span(
            &self,
            _kind: TraceSpanKind,
            _name: Cow<'static, str>,
            _fields: Value,
        ) -> TraceSpanHandle {
            panic!("unused in adaptor test")
        }

        async fn update_span(&self, _span: &TraceSpanHandle, _fields: Value) {
            panic!("unused in adaptor test")
        }

        async fn end_span(&self, _span: TraceSpanHandle, _outcome: TraceOutcome, _fields: Value) {
            panic!("unused in adaptor test")
        }

        async fn finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {
            panic!("unused in adaptor test")
        }

        async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {
            panic!("unused in adaptor test")
        }
    }

    struct TestRuntimeView {
        state_store: TestToolStateStore,
        tool_events: TestToolEventSink,
        trace_recorder: TestTraceRecorder,
        agent_context: TestAgentContext,
        interaction: TestInteractionHandle,
        hookers: TestHookerRegistry,
    }

    impl TestRuntimeView {
        fn new(response: InteractionResponse) -> Self {
            Self {
                state_store: TestToolStateStore,
                tool_events: TestToolEventSink,
                trace_recorder: TestTraceRecorder,
                agent_context: TestAgentContext::new(),
                interaction: TestInteractionHandle::new(response),
                hookers: TestHookerRegistry,
            }
        }
    }

    impl RuntimeView for TestRuntimeView {
        fn state_store(&self) -> &dyn ToolStateStore {
            &self.state_store
        }

        fn tool_events(&self) -> &dyn ToolEventSink {
            &self.tool_events
        }

        fn trace_recorder(&self) -> &dyn TraceRecorder {
            &self.trace_recorder
        }

        fn agent_context(&self) -> &dyn AgentContext {
            &self.agent_context
        }

        fn interaction(&self) -> &dyn InteractionHandle {
            &self.interaction
        }

        fn hookers(&self) -> &dyn HookerRegistry {
            &self.hookers
        }
    }

    #[tokio::test]
    async fn ask_user_round_trip_returns_final_pre_result() {
        let script_path = std::env::temp_dir().join(format!(
            "plugin_ask_user_round_trip_{}.py",
            std::process::id()
        ));
        std::fs::write(
            &script_path,
            r#"import json
import sys

payload = json.load(sys.stdin)
interaction = payload.get("interaction")
if interaction is None:
    json.dump(
        {
            "action": "ask_user",
            "request": {
                "kind": "text_input",
                "prompt": "who approved this?",
            },
            "continuation": {"step": "final"},
        },
        sys.stdout,
    )
else:
    value = interaction["response"].get("value")
    json.dump(
        {
            "action": "final",
            "result": "deny",
            "reason": f"approved by {value}",
        },
        sys.stdout,
    )
"#,
        )
        .unwrap();

        let adaptor = PluginToolHookerAdaptor::new(
            HookerId("plugin_pre_ask".to_string()),
            HookPointId("test-agent.Tool.bash.pre".to_string()),
            format!("python3 {}", script_path.display()),
            Value::Null,
        );
        let runtime = TestRuntimeView::new(InteractionResponse::Text {
            value: Some("alice".to_string()),
            display_value: None,
        });
        let input = PreToolHookInput {
            call: FinalToolCall {
                call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                input: json!({"command": "pwd"}),
            },
        };

        let output = adaptor
            .invoke_pre(&input, &HookInvokeMetadata::default(), &runtime)
            .await
            .unwrap();

        match output.primary {
            HookInvokePrimary::Pre(PreHookResult::Deny { reason }) => {
                assert_eq!(reason, "approved by alice");
            }
            other => panic!("unexpected output: {:?}", other),
        }

        let recorded_requests = runtime.interaction.recorded_requests();
        assert_eq!(recorded_requests.len(), 1);
        match &recorded_requests[0] {
            InteractionRequest::TextInput {
                prompt,
                source:
                    Some(InteractionSource::Hooker {
                        hooker_name,
                        hook_point,
                    }),
                is_secret: _, // Ignore is_secret in test
            } => {
                assert_eq!(prompt, "who approved this?");
                assert_eq!(hooker_name, "plugin_pre_ask");
                assert_eq!(hook_point, "test-agent.Tool.bash.pre");
            }
            other => panic!("unexpected interaction request: {:?}", other),
        }

        let _ = std::fs::remove_file(script_path);
    }

    #[test]
    fn ask_user_requires_continuation_field() {
        let adaptor = PluginToolHookerAdaptor::new(
            HookerId("plugin_pre_ask".to_string()),
            HookPointId("test-agent.Tool.bash.pre".to_string()),
            "python3 unused.py".to_string(),
            Value::Null,
        );

        let error = adaptor
            .parse_plugin_command_response(json!({
                "action": "ask_user",
                "request": {
                    "kind": "confirm",
                    "prompt": "continue?",
                    "source": null,
                }
            }))
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("response must contain field 'continuation'"));
    }

    #[test]
    fn ask_user_request_parses_minimal_confirm() {
        let adaptor = PluginToolHookerAdaptor::new(
            HookerId("plugin_pre_ask".to_string()),
            HookPointId("test-agent.Tool.bash.pre".to_string()),
            "python3 unused.py".to_string(),
            Value::Null,
        );

        let parsed = adaptor
            .parse_plugin_command_response(json!({
                "action": "ask_user",
                "request": {
                    "kind": "confirm",
                    "prompt": "continue?"
                },
                "continuation": {"step": 1}
            }))
            .unwrap();

        match parsed {
            PluginCommandResponse::AskUser(AskUserDirective {
                request: PluginAskUserRequest::Confirm { prompt, .. },
                continuation,
            }) => {
                assert_eq!(prompt, "continue?");
                assert_eq!(continuation, json!({"step": 1}));
            }
            other => panic!("unexpected parsed response: {:?}", other),
        }
    }

    #[test]
    fn final_responses_remain_backward_compatible() {
        let adaptor = PluginToolHookerAdaptor::new(
            HookerId("plugin_pre_ask".to_string()),
            HookPointId("test-agent.Tool.bash.pre".to_string()),
            "python3 unused.py".to_string(),
            Value::Null,
        );

        let legacy = adaptor
            .parse_plugin_command_response(json!({
                "result": "allow"
            }))
            .unwrap();
        let explicit = adaptor
            .parse_plugin_command_response(json!({
                "action": "final",
                "result": "allow"
            }))
            .unwrap();

        assert!(matches!(legacy, PluginCommandResponse::Final(_)));
        assert!(matches!(explicit, PluginCommandResponse::Final(_)));
    }

    /// 验证 plugin hooker 子进程超时 + kill 兜底链路：
    /// spawn 一个 `sleep 30` 子进程（模拟卡死），用 2s 超时，断言：
    /// 1. 返回 ToolExecutionError::Timeout（而非永久阻塞）；
    /// 2. 耗时在超时附近（~2s），证明是超时触发而非子进程自己退出；
    /// 3. kill_on_drop 在 future drop 后杀掉子进程（不验证残留进程，靠 kill_on_drop 语义保证）。
    #[tokio::test]
    async fn run_plugin_command_times_out_and_kills_hung_child() {
        let adaptor = PluginToolHookerAdaptor::new(
            HookerId("plugin_timeout".to_string()),
            HookPointId("test-agent.Tool.bash.pre".to_string()),
            "sleep 30".to_string(),
            Value::Null,
        );
        let payload = json!({"stage": "pre"});

        let start = std::time::Instant::now();
        let result = adaptor
            .run_plugin_command_with_timeout(&payload, Duration::from_secs(2))
            .await;
        let elapsed = start.elapsed();

        // 1. 返回 Timeout 错误（不是 Ok，不是其它 ExecutionFailed）
        match result {
            Err(ToolExecutionError::Timeout { timeout_ms }) => {
                assert_eq!(timeout_ms, 2000, "timeout_ms 应等于传入超时(2s=2000ms)");
            }
            other => panic!("expected Timeout, got: {:?}", other),
        }

        // 2. 耗时应在超时附近（2s），允许调度抖动，但远小于 sleep 30 的 30s。
        //    上界给 10s：超过则说明根本没被超时 kill（卡住了）。
        assert!(
            elapsed.as_secs() < 10,
            "超时应在 ~2s 触发，实际耗时 {:?}（疑似未被超时 kill）",
            elapsed
        );
        // 下界：至少要等到超时阈值，不能秒回（秒回说明子进程没卡住）
        assert!(
            elapsed.as_secs() >= 2,
            "应等到 2s 超时才返回，实际耗时 {:?}（疑似子进程未卡住）",
            elapsed
        );
    }

    /// 真实复现客户卡死：经 Rust hooker 调真实 audit.py，audit.py 的 L3 call_llm
    /// 被 monkeypatch（sitecustomize）永久阻塞（等效 http 超时失效），audit.py 子进程
    /// 卡死不退出。验证 Rust 侧超时 + kill 兜底能强杀卡死的真实 audit.py 子进程。
    ///
    /// 注意：依赖本仓库 plugins/hookers/audit_agent 下的 venv 与 audit.py 存在，
    /// CI 环境可能没有，故标 #[ignore]，手动 `cargo test -- --ignored` 跑。
    #[tokio::test]
    #[ignore]
    async fn rust_kills_real_hung_audit_py_via_hooker() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let audit_py = repo_root.join("plugins/hookers/audit_agent/audit.py");
        let venv_python =
            repo_root.join("plugins/hookers/audit_agent/audit_policy_checker/venv/bin/python3");
        let inject_dir = repo_root.join("plugins/tests/hookers/audit_agent/cases/hang-llm-repro");
        let cfg_dir = std::env::temp_dir().join("audit_hang_test_cfg");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg = cfg_dir.join("config.json");
        std::fs::write(
            &cfg,
            r#"{"llm":{"api_key":"sk-fake","model":"m","temperature":0.1,"base_url":"http://127.0.0.1:1/api/v1"},"timeout":{"total_timeout":60.0,"prompt1_timeout":30.0,"prompt2_timeout":20.0,"step_interval":0.0},"cache":{"enabled":false,"max_size":1000},"retry":{"max_retries":1,"retry_interval":1.0},"security":{"enabled":true,"heuristic_enabled":true,"logic_rules_enabled":true,"llm_analysis_enabled":true,"rules_path":"","skills_dir":""},"log_level":"INFO"}"#,
        )
        .unwrap();

        // command 经 sh -c，前缀设 env：注入 sitecustomize（patch call_llm 永久阻塞）+
        // 测试 config（开 L3）+ 短 L3 超时 + 隔离 HOME。venv python 跑真实 audit.py。
        let command = format!(
            "PYTHONPATH={inject} HOME={home} AUDIT_CONFIG_PATH={cfg} AUDIT_LLM_TIMEOUT=10 {py} {audit}",
            inject = inject_dir.display(),
            home = std::env::temp_dir().display(),
            cfg = cfg.display(),
            py = venv_python.display(),
            audit = audit_py.display(),
        );
        let adaptor = PluginToolHookerAdaptor::new(
            HookerId("audit_agent".to_string()),
            HookPointId("test-agent.Tool.file_write.pre".to_string()),
            command,
            Value::Null,
        );
        // 会进 L3 的 payload（file_write 非白名单）
        let payload = json!({
            "stage": "pre",
            "session_id": "s",
            "prompt_session": "写测试文件",
            "action_history": [],
            "call": {"call_id": "c", "tool_name": "file_write",
                     "input": {"file_path": "/tmp/test_hang.txt", "content": "hello"}}
        });

        // Rust 侧用 5s 超时（比 L3 的 10s 短），验证 Rust 先杀掉卡死的 audit.py。
        let start = std::time::Instant::now();
        let result = adaptor
            .run_plugin_command_with_timeout(&payload, Duration::from_secs(5))
            .await;
        let elapsed = start.elapsed();

        match result {
            Err(ToolExecutionError::Timeout { timeout_ms }) => {
                assert_eq!(timeout_ms, 5000, "应为 Rust 侧 5s 超时");
            }
            other => panic!("expected Timeout(Rust 杀掉卡死 audit.py), got: {:?}", other),
        }
        // Rust 超时附近返回（~5s），证明强杀了卡死的真实 audit.py 子进程，未永久阻塞。
        assert!(
            elapsed.as_secs() < 15,
            "Rust 应在 ~5s 超时杀掉 audit.py，实际 {:?}（疑似没杀掉）",
            elapsed
        );
        assert!(
            elapsed.as_secs() >= 5,
            "应等到 5s 超时，实际 {:?}（疑似 audit.py 没卡住）",
            elapsed
        );
    }
}
