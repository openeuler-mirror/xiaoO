struct StreamingTestProvider {
    capabilities: ProviderCapabilities,
}

impl StreamingTestProvider {
    fn new() -> Self {
        Self {
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: false,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "streaming-test".to_string(),
            },
        }
    }
}

#[async_trait]
impl LlmProvider for StreamingTestProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        panic!("streaming path should use complete_stream instead of complete");
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        on_chunk(StreamChunk {
            delta_text: Some("Hello".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        });
        on_chunk(StreamChunk {
            delta_text: Some(" world".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        });

        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some("Hello world".to_string()),
                reasoning_content: None,
                tool_calls: Vec::new(),
                usage: Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                    cached_tokens: 0,
                },
                stop_reason: StopReason::EndTurn,
            },
            kv_cache_chunk_hashes: vec![],
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

struct SequentialUsageProvider {
    capabilities: ProviderCapabilities,
    call_count: Arc<StdMutex<usize>>,
}

impl SequentialUsageProvider {
    fn new(call_count: Arc<StdMutex<usize>>) -> Self {
        Self {
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: false,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "sequential-usage-test".to_string(),
            },
            call_count,
        }
    }
}

#[async_trait]
impl LlmProvider for SequentialUsageProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        panic!("streaming path should use complete_stream instead of complete");
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let call_number = {
            let mut count = self
                .call_count
                .lock()
                .expect("provider call count mutex should not be poisoned");
            *count += 1;
            *count
        };

        let (text, usage) = if call_number == 1 {
            (
                "first turn".to_string(),
                Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2,
                    total_tokens: 5,
                    cached_tokens: 0,
                },
            )
        } else {
            (
                "second turn".to_string(),
                Usage {
                    prompt_tokens: 7,
                    completion_tokens: 1,
                    total_tokens: 8,
                    cached_tokens: 0,
                },
            )
        };

        on_chunk(StreamChunk {
            delta_text: Some(text.clone()),
            delta_reasoning: None,
            delta_tool_call: None,
        });

        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some(text),
                reasoning_content: None,
                tool_calls: Vec::new(),
                usage,
                stop_reason: StopReason::EndTurn,
            },
            kv_cache_chunk_hashes: vec![],
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

struct FixedPromptBuilder;

#[async_trait]
impl PromptBuilder for FixedPromptBuilder {
    async fn build(&self, input: PromptBuildInput) -> Result<PromptBuildResult, PromptBuildError> {
        Ok(PromptBuildResult {
            request: LlmRequest::new(input.messages),
            estimated_input_tokens: 0,
            system_parts: Vec::new(),
        })
    }
}

struct FixedBudgetPolicy {
    config: TokenBudgetConfig,
}

impl FixedBudgetPolicy {
    fn new(config: TokenBudgetConfig) -> Self {
        Self { config }
    }
}

impl TokenBudgetPolicy for FixedBudgetPolicy {
    fn total_budget(&self) -> usize {
        self.config.total_budget
    }

    fn reserved_for_output(&self) -> usize {
        self.config.reserved_for_output
    }

    fn reserved_for_system(&self) -> usize {
        self.config.reserved_for_system
    }

    fn hard_limit_ratio(&self) -> f64 {
        self.config.hard_limit_ratio
    }

    fn validate(&self) -> Result<(), BudgetError> {
        Ok(())
    }

    fn available_budget(&self) -> Result<usize, BudgetError> {
        Ok(self
            .config
            .total_budget
            .saturating_sub(self.config.reserved_for_output)
            .saturating_sub(self.config.reserved_for_system))
    }

    fn history_limit(&self) -> Result<usize, BudgetError> {
        self.available_budget()
    }
}

struct VisibleToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl ToolSpecView for VisibleToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}

fn dummy_visible_tools() -> Vec<Arc<dyn ToolSpecView>> {
    vec![Arc::new(VisibleToolSpec {
        id: ToolId("tool.bash".to_string()),
        name: ToolName("bash".to_string()),
        description: "Execute a shell command".to_string(),
        input_schema: InputSchemaRef {
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }),
        },
        output_contract: OutputContract {
            description: "Command output".to_string(),
        },
        effect_profile: EffectProfile {
            reads_filesystem: true,
            writes_filesystem: false,
            network_access: false,
            side_effects: true,
        },
    })]
}

fn test_runtime(provider: Arc<LlmProviderWrapper>) -> AgentRuntime {
    test_runtime_with_max_turns(provider, 4)
}

fn test_runtime_with_max_turns(provider: Arc<LlmProviderWrapper>, max_turns: u32) -> AgentRuntime {
    test_runtime_with_registry(provider, max_turns, Arc::new(EmptyToolRegistry::new()))
}

fn test_runtime_with_registry(
    provider: Arc<LlmProviderWrapper>,
    max_turns: u32,
    tool_registry: Arc<dyn ToolRegistry>,
) -> AgentRuntime {
    let prompt_builder: Arc<dyn PromptBuilder> = Arc::new(FixedPromptBuilder);
    let compression_pipeline: Arc<dyn CompressionPipeline> =
        Arc::new(compact::PassthroughCompressionPipeline::new());
    let skill_registry: Arc<dyn SkillRegistry> = Arc::new(EmptySkillRegistry::new());
    let budget_config = TokenBudgetConfig {
        total_budget: 4096,
        reserved_for_output: 512,
        reserved_for_system: 256,
        hard_limit_ratio: 1.0,
    };
    let budget_policy: Arc<dyn TokenBudgetPolicy> =
        Arc::new(FixedBudgetPolicy::new(budget_config.clone()));

    AgentRuntime::builder()
        .llm_provider(provider)
        .compression_pipeline(compression_pipeline)
        .prompt_builder(prompt_builder)
        .system_prompt("You are a coding agent.")
        .tool_registry(tool_registry)
        .skill_registry(skill_registry)
        .feature_flags(FeatureFlags::default())
        .max_turns(max_turns)
        .token_budget_config(budget_config)
        .token_budget_policy(budget_policy)
        .build()
        .expect("test runtime should build")
}

#[derive(Default)]
struct RecordingLoopEventSink {
    assistant_messages: Mutex<Vec<String>>,
}

impl RecordingLoopEventSink {
    fn take_assistant_messages(&self) -> Vec<String> {
        self.assistant_messages
            .lock()
            .expect("assistant message recorder mutex should not be poisoned")
            .clone()
    }
}

impl LoopEventSink for RecordingLoopEventSink {
    fn on_turn_start(&self, _agent_id: &AgentId, _turn: u32) {}

    fn on_assistant_message(&self, _agent_id: &AgentId, text: &str) {
        self.assistant_messages
            .lock()
            .expect("assistant message recorder mutex should not be poisoned")
            .push(text.to_string());
    }

    fn on_tool_result(&self, _agent_id: &AgentId, _event: &ToolResultEvent) {}

    fn on_loop_end(&self, _agent_id: &AgentId, _summary: &LoopEndSummary) {}
}

#[tokio::test]
async fn run_agent_loop_emits_streaming_assistant_snapshots() {
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(StreamingTestProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime(provider);
    let sink = Arc::new(RecordingLoopEventSink::default());
    let input = AgentLoopInput::new("hello")
        .with_agent_id(AgentId("test-agent".to_string()))
        .with_event_sink(sink.clone());
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

    let outcome = run_agent_loop(&runtime, &mut loop_state, input)
        .await
        .expect("streaming test loop should succeed");

    assert!(matches!(
        outcome,
        LoopRunResult::Complete(AgentOutcome::Complete { .. })
    ));
    // Throttling (STREAM_EMIT_MIN_DELTA_CHARS=32) suppresses in-stream
    // emits for short responses like "Hello world" (11 chars); the
    // post-stream flush emits the final text exactly once.
    let emitted = sink.take_assistant_messages();
    assert!(
        !emitted.is_empty(),
        "sink should receive at least the final flush emit"
    );
    assert_eq!(emitted.last().map(String::as_str), Some("Hello world"));
    assert_eq!(loop_state.token_usage.total_tokens, 5);
    assert_eq!(
        loop_state
            .messages
            .read()
            .last()
            .and_then(ChatMessage::text_content),
        Some("Hello world")
    );
}

#[tokio::test]
async fn run_agent_loop_applies_max_turns_per_run_not_session_total() {
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(StreamingTestProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime_with_max_turns(provider, 2);
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());
    loop_state.turn_count = 10;

    let outcome = run_agent_loop(
        &runtime,
        &mut loop_state,
        AgentLoopInput::new("revise plan"),
    )
    .await
    .expect("loop should complete despite prior session turns");

    assert!(matches!(
        outcome,
        LoopRunResult::Complete(AgentOutcome::Complete { .. })
    ));
    assert_eq!(loop_state.turn_count, 11);
}

#[tokio::test]
async fn run_agent_loop_surfaces_max_turns_when_final_turn_yields_text() {
    // Regression: with `max_turns = 1`, `build_messages` withholds tools
    // on turn 1 (the final turn) so the model replies with text only.
    // `decide` must still return `MaxTurnsReached` — not `Complete` — so
    // the `*.Session.lifecycle.state` hook payload carries
    // `outcome = "max_turns_reached"`. Previously the `MaxTurnsReached`
    // branch was only reachable when the assistant emitted tool calls,
    // which the no-tools final turn makes impossible.
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(StreamingTestProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime_with_max_turns(provider, 1);
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

    let outcome = run_agent_loop(&runtime, &mut loop_state, AgentLoopInput::new("hi"))
        .await
        .expect("loop should terminate at max_turns");

    match outcome {
        LoopRunResult::Complete(AgentOutcome::MaxTurnsReached { partial_reply, .. }) => {
            assert_eq!(partial_reply.as_deref(), Some("Hello world"));
            assert_eq!(loop_state.turn_count, 1);
        }
        _ => panic!("expected MaxTurnsReached, got a different outcome"),
    }
}

#[test]
fn loop_stop_rule_matches_only_configured_successful_tool() {
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(StreamingTestProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime(provider);
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());
    let input = AgentLoopInput::new("plan").with_stop_rules([LoopStopRule::AfterSuccessfulTool {
        tool_name: "todo_write".to_string(),
    }]);
    let estimator = TokenEstimator::new();
    let ctx = LoopContext {
        snapshot: runtime.snapshot(),
        state: &mut loop_state,
        input,
        turn: TurnState::new(1),
        estimator: &estimator,
    };
    let result = ToolExecutionResult::Completed {
        final_call: agent_types::tool::FinalToolCall {
            call_id: "call_1".to_string(),
            tool_name: "todo_write".to_string(),
            input: serde_json::json!({}),
            ..Default::default()
        },
        raw_outcome: RawToolOutcome::Success {
            output: "ok".to_string(),
        },
        pre_hook_results: Vec::new(),
        post_hook_results: Vec::new(),
    };
    let failed_result = ToolExecutionResult::Completed {
        final_call: agent_types::tool::FinalToolCall {
            call_id: "call_2".to_string(),
            tool_name: "todo_write".to_string(),
            input: serde_json::json!({}),
            ..Default::default()
        },
        raw_outcome: RawToolOutcome::Error {
            message: "bad input".to_string(),
        },
        pre_hook_results: Vec::new(),
        post_hook_results: Vec::new(),
    };

    assert!(should_stop_after_tool_result(&ctx, &result));
    assert!(!should_stop_after_tool_result(&ctx, &failed_result));
}

#[tokio::test]
async fn run_agent_loop_overwrites_token_usage_with_current_turn_usage() {
    let call_count = Arc::new(StdMutex::new(0));
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(SequentialUsageProvider::new(call_count)),
        None,
        None,
    ));
    let runtime = test_runtime(provider);
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

    run_agent_loop(&runtime, &mut loop_state, AgentLoopInput::new("first"))
        .await
        .expect("first loop run should succeed");
    assert_eq!(loop_state.token_usage.prompt_tokens, 3);
    assert_eq!(loop_state.token_usage.completion_tokens, 2);
    assert_eq!(loop_state.token_usage.total_tokens, 5);

    let outcome = run_agent_loop(&runtime, &mut loop_state, AgentLoopInput::new("second"))
        .await
        .expect("second loop run should succeed");

    assert_eq!(loop_state.token_usage.prompt_tokens, 7);
    assert_eq!(loop_state.token_usage.completion_tokens, 1);
    assert_eq!(loop_state.token_usage.total_tokens, 8);

    match outcome {
        LoopRunResult::Complete(AgentOutcome::Complete { token_usage, .. }) => {
            assert_eq!(token_usage.prompt_tokens, 7);
            assert_eq!(token_usage.completion_tokens, 1);
            assert_eq!(token_usage.total_tokens, 8);
        }
        _ => panic!("unexpected outcome"),
    }
}
