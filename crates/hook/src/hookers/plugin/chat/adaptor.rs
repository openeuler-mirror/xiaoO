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
mod tests {
    use super::*;
    use std::borrow::Cow;
    use std::path::PathBuf;

    use super::super::super::test_support::block_on;

    use agent_contracts::events::tool_events::ToolEventSink;
    use agent_contracts::hook::registry::HookerRegistry;
    use agent_contracts::interaction::handle::InteractionHandle;
    use agent_contracts::runtime::agent_context::{AgentContext, ConversationView};
    use agent_contracts::runtime::runtime_view::RuntimeView;
    use agent_contracts::tool::state::ToolStateStore;
    use agent_contracts::trace::{TraceOutcome, TraceRecorder, TraceSpanHandle, TraceSpanKind};
    use agent_types::chat::ModelRef;
    use agent_types::common::HookerId;
    use agent_types::common::{AgentMetadata, WorkspaceRef};
    use agent_types::events::ToolLifecycleEvent;
    use agent_types::hook::HookPointId;
    use agent_types::tool::execution_types::ToolExecutionError;
    use agent_types::tool::FinalToolCall;
    use agent_types::{ContentBlock, MessageRole};

    fn user_text(text: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        }
    }

    // ---- minimal test runtime view --------------------------------------

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

    struct TestInteractionHandle;
    #[async_trait]
    impl InteractionHandle for TestInteractionHandle {
        async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
            InteractionResponse::Confirmed { allowed: false }
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
            panic!("unused in chat adaptor test")
        }
        fn update(&self, _record: &agent_types::tool::ToolLifecycleRecord) {}
        fn finish(
            &self,
            _record: &agent_types::tool::ToolLifecycleRecord,
            _result: &agent_types::tool::execution_types::ToolExecutionResult,
        ) {
        }
        fn fail(
            &self,
            _record: &agent_types::tool::ToolLifecycleRecord,
            _error: &ToolExecutionError,
        ) {
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
            TraceSpanHandle::new("trace-test", "span-test", None)
        }
        async fn update_span(&self, _span: &TraceSpanHandle, _fields: Value) {}
        async fn end_span(&self, _span: TraceSpanHandle, _outcome: TraceOutcome, _fields: Value) {}
        async fn finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {}
        async fn force_finalize_trace(&self, _outcome: TraceOutcome, _fields: Value) {}
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
        fn new() -> Self {
            Self {
                state_store: TestToolStateStore,
                tool_events: TestToolEventSink,
                trace_recorder: TestTraceRecorder,
                agent_context: TestAgentContext::new(),
                interaction: TestInteractionHandle,
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

    fn adaptor_for(command: &str) -> PluginChatHookerAdaptor {
        PluginChatHookerAdaptor::new(
            HookerId("plugin_chat_test".to_string()),
            HookPointId("test-agent.Chat.system.transform".to_string()),
            command.to_string(),
            Value::Null,
        )
    }

    /// Write `json` to a temp file and return a `sh -c` command string that
    /// drains stdin to `/dev/null` then prints the file. Draining stdin avoids
    /// a broken-pipe race with `run_plugin_command`, which always writes a
    /// payload to the child's stdin even though these canned-response tests
    /// ignore it. A monotonic counter guarantees a unique filename even when
    /// two tests allocate one in the same nanosecond (which `SystemTime` could
    /// not, causing flaky cross-test collisions).
    fn cat_command_for(json: &str) -> String {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "xiaoo_chat_hook_test_{}_{}.json",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::write(&path, json).unwrap();
        format!("cat > /dev/null; cat {}", path.display())
    }

    // ---- system transform ----------------------------------------------

    #[test]
    fn system_transform_allow_keeps_parts() {
        let adaptor = adaptor_for(&cat_command_for(r#"{"result":"allow"}"#));
        let runtime = TestRuntimeView::new();
        let input = ChatSystemTransformInput {
            session_id: Some("s1".to_string()),
            model: ModelRef::default(),
            current_system: vec!["base".to_string()],
        };
        let output = block_on(adaptor.invoke_system_transform(
            &input,
            &HookInvokeMetadata::default(),
            &runtime,
        ))
        .unwrap();
        match output {
            HookInvokeOutput::ChatSystemTransform(ChatSystemTransformResult::Allow) => {}
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn system_transform_transform_replaces_parts() {
        let adaptor = adaptor_for(&cat_command_for(
            r#"{"result":"transform","system":["new","parts"]}"#,
        ));
        let runtime = TestRuntimeView::new();
        let input = ChatSystemTransformInput {
            session_id: Some("s1".to_string()),
            model: ModelRef::default(),
            current_system: vec!["base".to_string()],
        };
        let output = block_on(adaptor.invoke_system_transform(
            &input,
            &HookInvokeMetadata::default(),
            &runtime,
        ))
        .unwrap();
        match output {
            HookInvokeOutput::ChatSystemTransform(ChatSystemTransformResult::Transform {
                system,
            }) => {
                assert_eq!(system, vec!["new".to_string(), "parts".to_string()]);
            }
            other => panic!("expected Transform, got {:?}", other),
        }
    }

    // ---- chat message ---------------------------------------------------

    #[test]
    fn chat_message_accept_keeps_message() {
        let mut adaptor = adaptor_for(&cat_command_for(r#"{"result":"accept"}"#));
        adaptor
            .core
            .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
        let runtime = TestRuntimeView::new();
        let candidate = user_text("hello");
        let input = ChatMessageHookInput {
            session_id: "s1".to_string(),
            agent: Some("test-agent".to_string()),
            model: None,
            message_id: None,
            message: candidate,
            prior_message_count: 0,
        };
        let output =
            block_on(adaptor.invoke_chat_message(&input, &HookInvokeMetadata::default(), &runtime))
                .unwrap();
        match output {
            HookInvokeOutput::ChatMessage(ChatMessageHookResult::Accept) => {}
            other => panic!("expected Accept, got {:?}", other),
        }
    }

    #[test]
    fn chat_message_transform_replaces_message() {
        let mut adaptor = adaptor_for(&cat_command_for(
            r#"{"result":"transform","message":{"role":"user","blocks":[{"type":"text","text":"redacted"}],"timestamp_ms":0,"message_id":null,"api_usage_tokens":null,"reasoning_content":null,"estimated_tokens":null}}"#,
        ));
        adaptor
            .core
            .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
        let runtime = TestRuntimeView::new();
        let candidate = user_text("secret sk-abc");
        let input = ChatMessageHookInput {
            session_id: "s1".to_string(),
            agent: Some("test-agent".to_string()),
            model: None,
            message_id: None,
            message: candidate,
            prior_message_count: 0,
        };
        let output =
            block_on(adaptor.invoke_chat_message(&input, &HookInvokeMetadata::default(), &runtime))
                .unwrap();
        match output {
            HookInvokeOutput::ChatMessage(ChatMessageHookResult::Transform { message }) => {
                assert_eq!(message.blocks.len(), 1);
                match &message.blocks[0] {
                    ContentBlock::Text { text } => assert_eq!(text, "redacted"),
                    other => panic!("expected text block, got {:?}", other),
                }
            }
            other => panic!("expected Transform, got {:?}", other),
        }
    }

    // ---- command before -------------------------------------------------

    #[test]
    fn command_before_allow_keeps_body() {
        let mut adaptor = adaptor_for(&cat_command_for(r#"{"result":"allow"}"#));
        adaptor
            .core
            .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
        let runtime = TestRuntimeView::new();
        let input = CommandExecuteBeforeInput {
            command: "review".to_string(),
            session_id: "s1".to_string(),
            arguments: "src/main.rs".to_string(),
            body: "Review this carefully.\n\nsrc/main.rs".to_string(),
        };
        let output = block_on(adaptor.invoke_command_before(
            &input,
            &HookInvokeMetadata::default(),
            &runtime,
        ))
        .unwrap();
        match output {
            HookInvokeOutput::CommandExecuteBefore(CommandExecuteBeforeResult::Allow) => {}
            other => panic!("expected Allow, got {:?}", other),
        }
    }

    #[test]
    fn command_before_transform_rewrites_body() {
        let mut adaptor = adaptor_for(&cat_command_for(
            r#"{"result":"transform","body":"rewritten body"}"#,
        ));
        adaptor
            .core
            .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
        let runtime = TestRuntimeView::new();
        let input = CommandExecuteBeforeInput {
            command: "review".to_string(),
            session_id: "s1".to_string(),
            arguments: "".to_string(),
            body: "original".to_string(),
        };
        let output = block_on(adaptor.invoke_command_before(
            &input,
            &HookInvokeMetadata::default(),
            &runtime,
        ))
        .unwrap();
        match output {
            HookInvokeOutput::CommandExecuteBefore(CommandExecuteBeforeResult::Transform {
                body,
            }) => {
                assert_eq!(body, "rewritten body");
            }
            other => panic!("expected Transform, got {:?}", other),
        }
    }

    #[test]
    fn command_before_deny_carries_reason() {
        let mut adaptor = adaptor_for(&cat_command_for(
            r#"{"result":"deny","reason":"blocked by policy"}"#,
        ));
        adaptor
            .core
            .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
        let runtime = TestRuntimeView::new();
        let input = CommandExecuteBeforeInput {
            command: "deploy".to_string(),
            session_id: "s1".to_string(),
            arguments: "".to_string(),
            body: "deploy prod".to_string(),
        };
        let output = block_on(adaptor.invoke_command_before(
            &input,
            &HookInvokeMetadata::default(),
            &runtime,
        ))
        .unwrap();
        match output {
            HookInvokeOutput::CommandExecuteBefore(CommandExecuteBeforeResult::Deny { reason }) => {
                assert_eq!(reason, "blocked by policy");
            }
            other => panic!("expected Deny, got {:?}", other),
        }
    }
}
