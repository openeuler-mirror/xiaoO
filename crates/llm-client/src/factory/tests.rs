use super::config::LlmProviderConfig;
use super::create::create_llm_provider;

#[test]
fn test_config_builder() {
    let config = LlmProviderConfig::new("openai", "gpt-4o").with_api_key("test-key");
    assert_eq!(config.provider, "openai");
    assert_eq!(config.api_key, Some("test-key".into()));
    assert_eq!(config.model, "gpt-4o");
}

#[test]
fn test_unknown_provider() {
    let config = LlmProviderConfig::new("unknown", "some-model");
    let result = create_llm_provider(&config, None, None);
    assert!(result.is_err());
}

#[test]
fn test_missing_api_key() {
    let config = LlmProviderConfig::new("openai", "gpt-4o");
    let result = create_llm_provider(&config, None, None);
    assert!(result.is_err());
}

#[test]
fn test_ollama_no_key_required() {
    let config = LlmProviderConfig::new("ollama", "llama3");
    let result = create_llm_provider(&config, None, None);
    assert!(result.is_ok());
}

#[test]
fn test_openai_compatible_requires_api_base() {
    let config =
        LlmProviderConfig::new("openai-compatible", "custom-model").with_api_key("test-key");
    let result = create_llm_provider(&config, None, None);
    assert!(result.is_err());
    match result {
        Err(error) => assert!(error.to_string().contains("API base required")),
        Ok(_) => panic!("expected missing api_base to fail"),
    }
}

// ── LlmProviderWrapper hook integration tests ────────────────────────────────

mod wrapper_tests {
    use std::any::Any;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use agent_contracts::hook::{Hooker, HookerRegistry};
    use agent_contracts::runtime::runtime_view::RuntimeView;
    use agent_contracts::{LlmProvider, ProviderCapabilities};
    use agent_types::common::HookerId;
    use agent_types::hook::{HookInvokeError, HookInvokeInput, HookInvokeOutput, HookPointId};
    use agent_types::llm::{
        AssistantMessage, ChatMessage, ContentBlock, ErrorLlmHookResult, LlmError, LlmRequest,
        LlmResponse, MessageRole, PostLlmHookResult, PreLlmHookResult, ReasoningEffort, StopReason,
        StreamChunk, Usage,
    };
    use async_trait::async_trait;

    use super::super::wrapper::LlmProviderWrapper;

    // ── test data builders ───────────────────────────────────────────────────

    fn make_request(msg: &str) -> LlmRequest {
        LlmRequest {
            messages: vec![ChatMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: msg.to_string(),
                }],
                message_id: None,
                timestamp_ms: 0,
                api_usage_tokens: None,
                reasoning_content: None,
            }],
            tools: vec![],
            tool_choice: Default::default(),
            max_tokens: None,
            temperature: None,
            response_format: Default::default(),
            reasoning_effort: ReasoningEffort::Off,
        }
    }

    fn make_response(text: &str) -> LlmResponse {
        LlmResponse {
            message: AssistantMessage {
                text: Some(text.to_string()),
                reasoning_content: None,
                tool_calls: vec![],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                },
                stop_reason: StopReason::EndTurn,
            },
            kv_cache_chunk_hashes: vec![],
        }
    }

    fn default_caps() -> ProviderCapabilities {
        ProviderCapabilities {
            supports_streaming: true,
            supports_tool_calls: false,
            supports_json_mode: false,
            max_context_window: 4096,
            model_name: "test-model".to_string(),
        }
    }

    fn extract_text(request: &LlmRequest) -> &str {
        match &request.messages[0].blocks[0] {
            ContentBlock::Text { text } => text.as_str(),
            other => panic!("expected Text block, got {:?}", other),
        }
    }

    // ── mock LLM provider ────────────────────────────────────────────────────

    struct MockLlmProvider {
        captured: Mutex<Option<LlmRequest>>,
        result: Result<LlmResponse, LlmError>,
        caps: ProviderCapabilities,
    }

    impl MockLlmProvider {
        fn ok(resp: LlmResponse) -> Arc<Self> {
            Arc::new(Self {
                captured: Mutex::new(None),
                result: Ok(resp),
                caps: default_caps(),
            })
        }

        fn err(e: LlmError) -> Arc<Self> {
            Arc::new(Self {
                captured: Mutex::new(None),
                result: Err(e),
                caps: default_caps(),
            })
        }

        fn last_request(&self) -> Option<LlmRequest> {
            self.captured.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl LlmProvider for MockLlmProvider {
        async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            *self.captured.lock().unwrap() = Some(request.clone());
            self.result.clone()
        }

        async fn complete_stream(
            &self,
            request: &LlmRequest,
            _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            *self.captured.lock().unwrap() = Some(request.clone());
            self.result.clone()
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.caps
        }
    }

    struct SequencedMockLlmProvider {
        captured: Mutex<Vec<LlmRequest>>,
        results: Mutex<Vec<Result<LlmResponse, LlmError>>>,
        call_count: AtomicUsize,
        caps: ProviderCapabilities,
    }

    impl SequencedMockLlmProvider {
        fn new(results: Vec<Result<LlmResponse, LlmError>>) -> Arc<Self> {
            Arc::new(Self {
                captured: Mutex::new(Vec::new()),
                results: Mutex::new(results),
                call_count: AtomicUsize::new(0),
                caps: default_caps(),
            })
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl LlmProvider for SequencedMockLlmProvider {
        async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.captured.lock().unwrap().push(request.clone());
            self.results.lock().unwrap().remove(0)
        }

        async fn complete_stream(
            &self,
            request: &LlmRequest,
            _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.captured.lock().unwrap().push(request.clone());
            self.results.lock().unwrap().remove(0)
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.caps
        }
    }

    // ── mock hooker registry ─────────────────────────────────────────────────

    struct MockHookerRegistry {
        hookers: HashMap<HookerId, Box<dyn Hooker>>,
        enabled: HashSet<HookerId>,
    }

    impl MockHookerRegistry {
        fn with_hookers(hookers: Vec<Box<dyn Hooker>>) -> Self {
            let mut map = HashMap::new();
            let mut enabled = HashSet::new();
            for h in hookers {
                enabled.insert(h.id().clone());
                map.insert(h.id().clone(), h);
            }
            Self {
                hookers: map,
                enabled,
            }
        }

        fn empty() -> Self {
            Self::with_hookers(vec![])
        }
    }

    impl HookerRegistry for MockHookerRegistry {
        fn get(&self, id: &HookerId) -> Option<&dyn Hooker> {
            self.hookers.get(id).map(Box::as_ref)
        }
        fn list(&self) -> Vec<&dyn Hooker> {
            self.hookers.values().map(Box::as_ref).collect()
        }
        fn list_for_hook_point(&self, hp: &HookPointId) -> Vec<&dyn Hooker> {
            self.hookers
                .values()
                .filter(|h| h.hook_point() == hp)
                .map(Box::as_ref)
                .collect()
        }
        fn is_enabled(&self, id: &HookerId) -> bool {
            self.enabled.contains(id)
        }
        fn policy_for(&self, _id: &HookerId) -> Option<&serde_json::Value> {
            None
        }
    }

    // ── minimal test runtime view ────────────────────────────────────────────
    // Only hookers() is used by LlmProviderWrapper; all other methods panic.

    struct TestRuntimeView {
        registry: MockHookerRegistry,
        trace_recorder: TestTraceRecorder,
        agent_context: TestAgentContext,
    }

    impl TestRuntimeView {
        fn new(registry: MockHookerRegistry) -> Arc<Self> {
            Arc::new(Self {
                registry,
                trace_recorder: TestTraceRecorder,
                agent_context: TestAgentContext::default(),
            })
        }
    }

    struct TestTraceRecorder;

    #[async_trait]
    impl agent_contracts::TraceRecorder for TestTraceRecorder {
        async fn begin_span(
            &self,
            _kind: agent_contracts::TraceSpanKind,
            _name: std::borrow::Cow<'static, str>,
            _fields: serde_json::Value,
        ) -> agent_contracts::TraceSpanHandle {
            agent_contracts::TraceSpanHandle::new("", "", None)
        }

        async fn update_span(
            &self,
            _span: &agent_contracts::TraceSpanHandle,
            _fields: serde_json::Value,
        ) {
        }

        async fn end_span(
            &self,
            _span: agent_contracts::TraceSpanHandle,
            _outcome: agent_contracts::TraceOutcome,
            _fields: serde_json::Value,
        ) {
        }

        async fn finalize_trace(
            &self,
            _outcome: agent_contracts::TraceOutcome,
            _fields: serde_json::Value,
        ) {
        }

        async fn force_finalize_trace(
            &self,
            _outcome: agent_contracts::TraceOutcome,
            _fields: serde_json::Value,
        ) {
        }
    }

    impl RuntimeView for TestRuntimeView {
        fn state_store(&self) -> &dyn agent_contracts::ToolStateStore {
            panic!("not used in llm hook tests")
        }
        fn tool_events(&self) -> &dyn agent_contracts::ToolEventSink {
            panic!("not used in llm hook tests")
        }
        fn trace_recorder(&self) -> &dyn agent_contracts::TraceRecorder {
            &self.trace_recorder
        }
        fn agent_context(&self) -> &dyn agent_contracts::AgentContext {
            &self.agent_context
        }
        fn interaction(&self) -> &dyn agent_contracts::InteractionHandle {
            panic!("not used in llm hook tests")
        }
        fn hookers(&self) -> &dyn HookerRegistry {
            &self.registry
        }
    }

    #[derive(Default)]
    struct TestConversationView {
        messages: Vec<ChatMessage>,
    }

	impl agent_contracts::ConversationView for TestConversationView {
		fn recent_messages(&self, _limit: usize) -> Vec<ChatMessage> {
			self.messages.clone()
		}

        fn message_count(&self) -> usize {
            self.messages.len()
        }
    }

    struct TestAgentContext {
        conversation: TestConversationView,
        workspace: agent_types::common::WorkspaceRef,
        metadata: agent_types::common::AgentMetadata,
    }

    impl Default for TestAgentContext {
        fn default() -> Self {
            Self {
                conversation: TestConversationView::default(),
                workspace: agent_types::common::WorkspaceRef {
                    root: std::path::PathBuf::from("."),
                },
                metadata: agent_types::common::AgentMetadata {
                    agent_id: "test-agent".to_string(),
                    model: "test-model".to_string(),
                    session_id: Some("test-session".to_string()),
                },
            }
        }
    }

    impl agent_contracts::AgentContext for TestAgentContext {
        fn conversation(&self) -> &dyn agent_contracts::ConversationView {
            &self.conversation
        }

        fn workspace(&self) -> &agent_types::common::WorkspaceRef {
            &self.workspace
        }

        fn metadata(&self) -> &agent_types::common::AgentMetadata {
            &self.metadata
        }
    }

    // ── test hooker implementations ──────────────────────────────────────────

    /// Pre-hook: returns Allow, leaving the request unchanged.
    struct AllowPreHooker {
        id: HookerId,
        hook_point: HookPointId,
    }

    impl AllowPreHooker {
        fn boxed(agent_id: &str) -> Box<dyn Hooker> {
            Box::new(Self {
                id: HookerId("test_allow_pre".to_string()),
                hook_point: HookPointId(format!("{}.Llm.complete.pre", agent_id)),
            })
        }
    }

    #[async_trait]
    impl Hooker for AllowPreHooker {
        fn id(&self) -> &HookerId {
            &self.id
        }
        fn hook_point(&self) -> &HookPointId {
            &self.hook_point
        }
        async fn invoke(
            &self,
            input: HookInvokeInput,
            _: &dyn RuntimeView,
        ) -> Result<HookInvokeOutput, HookInvokeError> {
            match input {
                HookInvokeInput::LlmPre { .. } => {
                    Ok(HookInvokeOutput::LlmPre(PreLlmHookResult::Allow))
                }
                _ => panic!("AllowPreHooker: unexpected input"),
            }
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Pre-hook: replaces all messages with a single configured message.
    struct TransformPreHooker {
        id: HookerId,
        hook_point: HookPointId,
        new_msg: String,
    }

    impl TransformPreHooker {
        fn boxed(agent_id: &str, new_msg: &str) -> Box<dyn Hooker> {
            Box::new(Self {
                id: HookerId("test_transform_pre".to_string()),
                hook_point: HookPointId(format!("{}.Llm.complete.pre", agent_id)),
                new_msg: new_msg.to_string(),
            })
        }
    }

    #[async_trait]
    impl Hooker for TransformPreHooker {
        fn id(&self) -> &HookerId {
            &self.id
        }
        fn hook_point(&self) -> &HookPointId {
            &self.hook_point
        }
        async fn invoke(
            &self,
            input: HookInvokeInput,
            _: &dyn RuntimeView,
        ) -> Result<HookInvokeOutput, HookInvokeError> {
            match input {
                HookInvokeInput::LlmPre {
                    input: pre_input, ..
                } => {
                    let mut req = pre_input.request;
                    req.messages = vec![ChatMessage {
                        role: MessageRole::User,
                        blocks: vec![ContentBlock::Text {
                            text: self.new_msg.clone(),
                        }],
                        message_id: None,
                        timestamp_ms: 0,
                        api_usage_tokens: None,
                        reasoning_content: None,
                    }];
                    Ok(HookInvokeOutput::LlmPre(PreLlmHookResult::Transform {
                        modified_request: req,
                    }))
                }
                _ => panic!("TransformPreHooker: unexpected input"),
            }
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Post-hook: returns Accept, leaving the response unchanged.
    struct AcceptPostHooker {
        id: HookerId,
        hook_point: HookPointId,
    }

    impl AcceptPostHooker {
        fn boxed(agent_id: &str) -> Box<dyn Hooker> {
            Box::new(Self {
                id: HookerId("test_accept_post".to_string()),
                hook_point: HookPointId(format!("{}.Llm.complete.post", agent_id)),
            })
        }
    }

    #[async_trait]
    impl Hooker for AcceptPostHooker {
        fn id(&self) -> &HookerId {
            &self.id
        }
        fn hook_point(&self) -> &HookPointId {
            &self.hook_point
        }
        async fn invoke(
            &self,
            input: HookInvokeInput,
            _: &dyn RuntimeView,
        ) -> Result<HookInvokeOutput, HookInvokeError> {
            match input {
                HookInvokeInput::LlmPost { .. } => {
                    Ok(HookInvokeOutput::LlmPost(PostLlmHookResult::Accept))
                }
                _ => panic!("AcceptPostHooker: unexpected input"),
            }
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Post-hook: replaces the response text with a configured string.
    struct TransformPostHooker {
        id: HookerId,
        hook_point: HookPointId,
        new_text: String,
    }

    impl TransformPostHooker {
        fn boxed(agent_id: &str, new_text: &str) -> Box<dyn Hooker> {
            Box::new(Self {
                id: HookerId("test_transform_post".to_string()),
                hook_point: HookPointId(format!("{}.Llm.complete.post", agent_id)),
                new_text: new_text.to_string(),
            })
        }
    }

    #[async_trait]
    impl Hooker for TransformPostHooker {
        fn id(&self) -> &HookerId {
            &self.id
        }
        fn hook_point(&self) -> &HookPointId {
            &self.hook_point
        }
        async fn invoke(
            &self,
            input: HookInvokeInput,
            _: &dyn RuntimeView,
        ) -> Result<HookInvokeOutput, HookInvokeError> {
            match input {
                HookInvokeInput::LlmPost {
                    input: post_input, ..
                } => {
                    let mut resp = post_input.response;
                    resp.message.text = Some(self.new_text.clone());
                    Ok(HookInvokeOutput::LlmPost(PostLlmHookResult::Transform {
                        modified_response: resp,
                    }))
                }
                _ => panic!("TransformPostHooker: unexpected input"),
            }
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Error-hook: returns Propagate, letting the original error through.
    struct PropagateErrorHooker {
        id: HookerId,
        hook_point: HookPointId,
    }

    impl PropagateErrorHooker {
        fn boxed(agent_id: &str) -> Box<dyn Hooker> {
            Box::new(Self {
                id: HookerId("test_propagate_error".to_string()),
                hook_point: HookPointId(format!("{}.Llm.complete.error", agent_id)),
            })
        }
    }

    #[async_trait]
    impl Hooker for PropagateErrorHooker {
        fn id(&self) -> &HookerId {
            &self.id
        }
        fn hook_point(&self) -> &HookPointId {
            &self.hook_point
        }
        async fn invoke(
            &self,
            input: HookInvokeInput,
            _: &dyn RuntimeView,
        ) -> Result<HookInvokeOutput, HookInvokeError> {
            match input {
                HookInvokeInput::LlmError { .. } => {
                    Ok(HookInvokeOutput::LlmError(ErrorLlmHookResult::Propagate))
                }
                _ => panic!("PropagateErrorHooker: unexpected input"),
            }
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Error-hook: returns Recover with a configured fallback response.
    struct RecoverErrorHooker {
        id: HookerId,
        hook_point: HookPointId,
        fallback_text: String,
    }

    impl RecoverErrorHooker {
        fn boxed(agent_id: &str, fallback_text: &str) -> Box<dyn Hooker> {
            Box::new(Self {
                id: HookerId("test_recover_error".to_string()),
                hook_point: HookPointId(format!("{}.Llm.complete.error", agent_id)),
                fallback_text: fallback_text.to_string(),
            })
        }
    }

    #[async_trait]
    impl Hooker for RecoverErrorHooker {
        fn id(&self) -> &HookerId {
            &self.id
        }
        fn hook_point(&self) -> &HookPointId {
            &self.hook_point
        }
        async fn invoke(
            &self,
            input: HookInvokeInput,
            _: &dyn RuntimeView,
        ) -> Result<HookInvokeOutput, HookInvokeError> {
            match input {
                HookInvokeInput::LlmError { .. } => {
                    Ok(HookInvokeOutput::LlmError(ErrorLlmHookResult::Recover {
                        response: make_response(&self.fallback_text),
                    }))
                }
                _ => panic!("RecoverErrorHooker: unexpected input"),
            }
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    // ── wrapper builder helpers ──────────────────────────────────────────────

    fn wrapper_with_hooks(
        provider: Arc<dyn LlmProvider>,
        hooks: Vec<Box<dyn Hooker>>,
        agent_id: &str,
    ) -> LlmProviderWrapper {
        let registry = MockHookerRegistry::with_hookers(hooks);
        let runtime = TestRuntimeView::new(registry);
        LlmProviderWrapper::new(provider, Some(agent_id.to_string()), Some(runtime))
    }

    fn wrapper_without_runtime(provider: Arc<dyn LlmProvider>) -> LlmProviderWrapper {
        LlmProviderWrapper::new(provider, None, None)
    }

    // ── complete() tests ─────────────────────────────────────────────────────

    /// Without a runtime view, the inner provider is called directly.
    #[tokio::test]
    async fn no_runtime_view_passes_request_through() {
        let mock = MockLlmProvider::ok(make_response("direct"));
        let wrapper = wrapper_without_runtime(mock.clone());
        let result = wrapper.complete(&make_request("hello")).await.unwrap();
        assert_eq!(result.message.text.as_deref(), Some("direct"));
        let captured = mock.last_request().unwrap();
        assert_eq!(extract_text(&captured), "hello");
    }

    /// An empty registry means no hooks fire; provider receives the original request.
    #[tokio::test]
    async fn empty_registry_request_unchanged() {
        let mock = MockLlmProvider::ok(make_response("ok"));
        let wrapper = wrapper_with_hooks(mock.clone(), vec![], "agent");
        wrapper.complete(&make_request("original")).await.unwrap();
        assert_eq!(extract_text(&mock.last_request().unwrap()), "original");
    }

    /// A pre-hook returning Allow does not modify the request.
    #[tokio::test]
    async fn pre_hook_allow_request_unchanged() {
        let mock = MockLlmProvider::ok(make_response("ok"));
        let wrapper =
            wrapper_with_hooks(mock.clone(), vec![AllowPreHooker::boxed("agent")], "agent");
        wrapper.complete(&make_request("original")).await.unwrap();
        assert_eq!(extract_text(&mock.last_request().unwrap()), "original");
    }

    /// A pre-hook returning Transform replaces the request seen by the inner provider.
    #[tokio::test]
    async fn pre_hook_transform_modifies_request_seen_by_provider() {
        let mock = MockLlmProvider::ok(make_response("ok"));
        let wrapper = wrapper_with_hooks(
            mock.clone(),
            vec![TransformPreHooker::boxed("agent", "transformed")],
            "agent",
        );
        wrapper.complete(&make_request("original")).await.unwrap();
        assert_eq!(extract_text(&mock.last_request().unwrap()), "transformed");
    }

    /// A post-hook returning Accept leaves the response unchanged.
    #[tokio::test]
    async fn post_hook_accept_preserves_response() {
        let mock = MockLlmProvider::ok(make_response("original-text"));
        let wrapper = wrapper_with_hooks(mock, vec![AcceptPostHooker::boxed("agent")], "agent");
        let result = wrapper.complete(&make_request("hi")).await.unwrap();
        assert_eq!(result.message.text.as_deref(), Some("original-text"));
    }

    /// A post-hook returning Transform replaces the response returned to the caller.
    #[tokio::test]
    async fn post_hook_transform_replaces_response() {
        let mock = MockLlmProvider::ok(make_response("original-text"));
        let wrapper = wrapper_with_hooks(
            mock,
            vec![TransformPostHooker::boxed("agent", "transformed-text")],
            "agent",
        );
        let result = wrapper.complete(&make_request("hi")).await.unwrap();
        assert_eq!(result.message.text.as_deref(), Some("transformed-text"));
    }

    /// An error-hook returning Propagate does not suppress the error.
    #[tokio::test]
    async fn error_hook_propagate_returns_original_error() {
        let mock = MockLlmProvider::err(LlmError::ApiError("upstream down".to_string()));
        let wrapper = wrapper_with_hooks(mock, vec![PropagateErrorHooker::boxed("agent")], "agent");
        let err = wrapper.complete(&make_request("hi")).await.unwrap_err();
        assert!(err.to_string().contains("upstream down"));
    }

    /// An error-hook returning Recover returns the fallback response instead of the error.
    #[tokio::test]
    async fn error_hook_recover_returns_fallback_response() {
        let mock = MockLlmProvider::err(LlmError::ApiError("upstream down".to_string()));
        let wrapper = wrapper_with_hooks(
            mock,
            vec![RecoverErrorHooker::boxed("agent", "fallback")],
            "agent",
        );
        let result = wrapper.complete(&make_request("hi")).await.unwrap();
        assert_eq!(result.message.text.as_deref(), Some("fallback"));
    }

    /// With no error hooks registered, errors propagate naturally.
    #[tokio::test]
    async fn no_error_hook_propagates_error() {
        let mock = MockLlmProvider::err(LlmError::Timeout);
        let wrapper = wrapper_with_hooks(mock, vec![], "agent");
        let err = wrapper.complete(&make_request("hi")).await.unwrap_err();
        assert!(matches!(err, LlmError::Timeout));
    }

    #[tokio::test]
    async fn rate_limited_complete_retries_once_and_succeeds() {
        let mock = SequencedMockLlmProvider::new(vec![
            Err(LlmError::RateLimited {
                retry_after_ms: 1,
                message: "busy".to_string(),
            }),
            Ok(make_response("recovered")),
        ]);
        let wrapper = wrapper_without_runtime(mock.clone());

        let result = wrapper.complete(&make_request("hi")).await.unwrap();

        assert_eq!(result.message.text.as_deref(), Some("recovered"));
        assert_eq!(mock.call_count(), 2);
    }

    /// A registered but disabled hooker must not fire.
    #[tokio::test]
    async fn disabled_pre_hook_does_not_modify_request() {
        let hook = TransformPreHooker::boxed("agent", "should-not-appear");
        // Build registry then clear enabled set to simulate disabled state.
        let mut registry = MockHookerRegistry::with_hookers(vec![hook]);
        registry.enabled.clear();

        let mock = MockLlmProvider::ok(make_response("ok"));
        let runtime = TestRuntimeView::new(registry);
        let wrapper =
            LlmProviderWrapper::new(mock.clone(), Some("agent".to_string()), Some(runtime));
        wrapper.complete(&make_request("original")).await.unwrap();
        assert_eq!(extract_text(&mock.last_request().unwrap()), "original");
    }

    /// Multiple transform pre-hooks execute sequentially in hook-id alphabetical order.
    /// The last hook in the chain wins.
    #[tokio::test]
    async fn chained_pre_hooks_apply_in_order() {
        struct NamedTransformPre {
            id: HookerId,
            hook_point: HookPointId,
            new_msg: String,
        }

        #[async_trait]
        impl Hooker for NamedTransformPre {
            fn id(&self) -> &HookerId {
                &self.id
            }
            fn hook_point(&self) -> &HookPointId {
                &self.hook_point
            }
            async fn invoke(
                &self,
                input: HookInvokeInput,
                _: &dyn RuntimeView,
            ) -> Result<HookInvokeOutput, HookInvokeError> {
                match input {
                    HookInvokeInput::LlmPre { input: pre, .. } => {
                        let mut req = pre.request;
                        req.messages = vec![ChatMessage {
                            role: MessageRole::User,
                            blocks: vec![ContentBlock::Text {
                                text: self.new_msg.clone(),
                            }],
                            message_id: None,
                            timestamp_ms: 0,
                            api_usage_tokens: None,
                            reasoning_content: None,
                        }];
                        Ok(HookInvokeOutput::LlmPre(PreLlmHookResult::Transform {
                            modified_request: req,
                        }))
                    }
                    _ => panic!("NamedTransformPre: unexpected input"),
                }
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }

        // IDs are sorted alphabetically; "aaa_" sorts before "zzz_".
        let hook_a: Box<dyn Hooker> = Box::new(NamedTransformPre {
            id: HookerId("aaa_first".to_string()),
            hook_point: HookPointId("agent.Llm.complete.pre".to_string()),
            new_msg: "after-first".to_string(),
        });
        let hook_b: Box<dyn Hooker> = Box::new(NamedTransformPre {
            id: HookerId("zzz_second".to_string()),
            hook_point: HookPointId("agent.Llm.complete.pre".to_string()),
            new_msg: "after-second".to_string(),
        });

        let mock = MockLlmProvider::ok(make_response("done"));
        let wrapper = wrapper_with_hooks(mock.clone(), vec![hook_a, hook_b], "agent");
        wrapper.complete(&make_request("original")).await.unwrap();

        // second hook ran last and produced "after-second"
        assert_eq!(extract_text(&mock.last_request().unwrap()), "after-second");
    }

    // ── complete_stream() tests ──────────────────────────────────────────────

    /// Stream: pre-hook transform modifies the request before the inner provider is called.
    #[tokio::test]
    async fn stream_pre_hook_transform_modifies_request() {
        let mock = MockLlmProvider::ok(make_response("stream-ok"));
        let wrapper = wrapper_with_hooks(
            mock.clone(),
            vec![TransformPreHooker::boxed("agent", "stream-transformed")],
            "agent",
        );
        wrapper
            .complete_stream(&make_request("original"), &|_| {})
            .await
            .unwrap();
        assert_eq!(
            extract_text(&mock.last_request().unwrap()),
            "stream-transformed"
        );
    }

    /// Stream: post-hook transform replaces the aggregated response.
    #[tokio::test]
    async fn stream_post_hook_transform_replaces_response() {
        let mock = MockLlmProvider::ok(make_response("stream-original"));
        let wrapper = wrapper_with_hooks(
            mock,
            vec![TransformPostHooker::boxed("agent", "stream-modified")],
            "agent",
        );
        let result = wrapper
            .complete_stream(&make_request("hi"), &|_| {})
            .await
            .unwrap();
        assert_eq!(result.message.text.as_deref(), Some("stream-modified"));
    }

    /// Stream: error-hook Recover returns the fallback instead of propagating the stream error.
    #[tokio::test]
    async fn stream_error_hook_recover_returns_fallback() {
        let mock = MockLlmProvider::err(LlmError::StreamError {
            message: "broken pipe".to_string(),
        });
        let wrapper = wrapper_with_hooks(
            mock,
            vec![RecoverErrorHooker::boxed("agent", "stream-fallback")],
            "agent",
        );
        let result = wrapper
            .complete_stream(&make_request("hi"), &|_| {})
            .await
            .unwrap();
        assert_eq!(result.message.text.as_deref(), Some("stream-fallback"));
    }

    /// Stream: without a runtime view, errors propagate directly.
    #[tokio::test]
    async fn stream_no_runtime_error_propagates() {
        let mock = MockLlmProvider::err(LlmError::Timeout);
        let wrapper = wrapper_without_runtime(mock);
        let err = wrapper
            .complete_stream(&make_request("hi"), &|_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, LlmError::Timeout));
    }

    #[tokio::test]
    async fn rate_limited_stream_retries_once_and_succeeds() {
        let mock = SequencedMockLlmProvider::new(vec![
            Err(LlmError::RateLimited {
                retry_after_ms: 1,
                message: "busy".to_string(),
            }),
            Ok(make_response("stream-recovered")),
        ]);
        let wrapper = wrapper_without_runtime(mock.clone());

        let result = wrapper
            .complete_stream(&make_request("hi"), &|_| {})
            .await
            .unwrap();

        assert_eq!(result.message.text.as_deref(), Some("stream-recovered"));
        assert_eq!(mock.call_count(), 2);
    }
}
