/// Streams an empty-`call_id` tool call on the first turn, then a plain
/// completion so the loop terminates after the call is executed.
struct EmptyCallIdToolCallProvider {
    capabilities: ProviderCapabilities,
    calls: Arc<StdMutex<usize>>,
}

impl EmptyCallIdToolCallProvider {
    fn new() -> Self {
        Self {
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: true,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "empty-call-id-test".to_string(),
            },
            calls: Arc::new(StdMutex::new(0)),
        }
    }
}

#[async_trait]
impl LlmProvider for EmptyCallIdToolCallProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        panic!("streaming path should use complete_stream instead of complete");
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let call_number = {
            let mut calls = self.calls.lock().expect("call counter mutex poisoned");
            *calls += 1;
            *calls
        };

        on_chunk(StreamChunk {
            delta_text: Some("trying to use a tool".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        });

        if call_number >= 2 {
            return Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some("done".to_string()),
                    reasoning_content: None,
                    tool_calls: Vec::new(),
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 2,
                        total_tokens: 12,
                        cached_tokens: 0,
                    },
                    stop_reason: StopReason::EndTurn,
                },
                kv_cache_chunk_hashes: vec![],
            });
        }

        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some("trying to use a tool".to_string()),
                reasoning_content: None,
                tool_calls: vec![ToolUseBlock {
                    call_id: String::new(),
                    tool_name: "bash".to_string(),
                    input: serde_json::json!({"command": "date"}),
                }],
                usage: Usage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cached_tokens: 0,
                },
                stop_reason: StopReason::ToolUse,
            },
            kv_cache_chunk_hashes: vec![],
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

#[tokio::test]
async fn run_agent_loop_synthesizes_missing_call_id_and_executes_tool() {
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(EmptyCallIdToolCallProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime(provider);
    let input = AgentLoopInput::new("现在几点")
        .with_agent_id(AgentId("test-agent".to_string()))
        .with_visible_tools(dummy_visible_tools())
        .with_runtime_view(Arc::new(NoopRuntimeView::new()));
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

    let outcome = run_agent_loop(&runtime, &mut loop_state, input)
        .await
        .expect("loop should execute the call after synthesizing its id");

    assert!(matches!(
        outcome,
        LoopRunResult::Complete(AgentOutcome::Complete { .. })
    ));
    // turn 1: synthesize + run the tool; turn 2: model stops and the loop
    // completes without injecting a checklist that could replace the answer.
    assert_eq!(loop_state.turn_count, 2);

    let messages = loop_state.messages.read();
    assert!(
        messages
            .iter()
            .filter_map(ChatMessage::text_content)
            .all(|text| !text.contains("You are about to finish")),
        "completion should not inject a checklist as a new user request"
    );
    let tool_use = messages.iter().find_map(|m| {
        m.blocks.iter().find_map(|b| match b {
            ContentBlock::ToolUse {
                call_id, tool_name, ..
            } => Some((call_id.clone(), tool_name.clone())),
            _ => None,
        })
    });
    assert_eq!(
        tool_use,
        Some(("call_0_0".to_string(), "bash".to_string())),
        "empty call_id should be synthesized to a stable turn-scoped id and preserved"
    );
    let paired = messages.iter().any(|m| {
        matches!(m.role, MessageRole::Tool)
            && m.blocks.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::ToolResult { call_id, .. } if call_id == "call_0_0"
                )
            })
    });
    assert!(
        paired,
        "synthesized tool_use must pair with a tool_result on the same id"
    );
}

#[test]
fn test_filter_ask_user_question_output() {
    // Test with display_value
    let input_with_display = json!({
        "answers": [{
            "kind": "text",
            "prompt": "Enter password",
            "value": "real_password_123",
            "display_value": "<SECRET>"
        }]
    })
    .to_string();

    let filtered = filter_ask_user_question_output(&input_with_display);

    // Should replace value with display_value
    let filtered_json: serde_json::Value = serde_json::from_str(&filtered).unwrap();
    assert_eq!(filtered_json["answers"][0]["value"], "<SECRET>");
    assert!(filtered_json["answers"][0].get("display_value").is_none());

    // Test without display_value
    let input_without_display = json!({
        "answers": [{
            "kind": "text",
            "prompt": "Enter name",
            "value": "John"
        }]
    })
    .to_string();

    let filtered2 = filter_ask_user_question_output(&input_without_display);
    let filtered2_json: serde_json::Value = serde_json::from_str(&filtered2).unwrap();
    assert_eq!(filtered2_json["answers"][0]["value"], "John");

    // Test with non-text type
    let input_choice = json!({
        "answers": [{
            "kind": "choice",
            "prompt": "Select option",
            "value": "option1"
        }]
    })
    .to_string();

    let filtered3 = filter_ask_user_question_output(&input_choice);
    let filtered3_json: serde_json::Value = serde_json::from_str(&filtered3).unwrap();
    assert_eq!(filtered3_json["answers"][0]["value"], "option1");

    // Test with invalid JSON
    let invalid = "not a json";
    let filtered4 = filter_ask_user_question_output(invalid);
    assert_eq!(filtered4, "not a json");
}

#[test]
fn test_extract_secrets_from_messages() {
    // Test 1: Only extract secrets with display_value (is_secret=true)
    let message_with_secret = ChatMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            call_id: "call_1".to_string(),
            tool_name: "ask_user_question".to_string(),
            output: json!({
                "answers": [{
                    "kind": "text",
                    "prompt": "Password",
                    "value": "secret123",
                    "display_value": "<SECRET>"
                }]
            })
            .to_string(),
            is_error: false,
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    };

    let message_with_normal_text = ChatMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            call_id: "call_2".to_string(),
            tool_name: "ask_user_question".to_string(),
            output: json!({
                "answers": [{
                    "kind": "text",
                    "prompt": "Username",
                    "value": "john"
                }]
            })
            .to_string(),
            is_error: false,
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    };

    let messages = vec![message_with_secret, message_with_normal_text];
    let secrets = extract_secrets_from_messages(&messages);

    // Should only extract the secret with display_value
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0], "secret123");
    assert!(!secrets.contains(&"john".to_string()));

    // Test 2: Multiple secrets in one answer
    let message_multiple = ChatMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            call_id: "call_3".to_string(),
            tool_name: "ask_user_question".to_string(),
            output: json!({
                "answers": [
                    {
                        "kind": "text",
                        "prompt": "Username",
                        "value": "admin"
                    },
                    {
                        "kind": "text",
                        "prompt": "Password",
                        "value": "pass123",
                        "display_value": "<SECRET>"
                    }
                ]
            })
            .to_string(),
            is_error: false,
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    };

    let secrets2 = extract_secrets_from_messages(&vec![message_multiple]);
    assert_eq!(secrets2.len(), 1);
    assert_eq!(secrets2[0], "pass123");
    assert!(!secrets2.contains(&"admin".to_string()));

    // Test 3: Non-ask_user_question tool results should not be extracted
    let message_other_tool = ChatMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            call_id: "call_4".to_string(),
            tool_name: "bash".to_string(),
            output: "some output with password123".to_string(),
            is_error: false,
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    };

    let secrets3 = extract_secrets_from_messages(&vec![message_other_tool]);
    assert_eq!(secrets3.len(), 0);
}

#[test]
fn is_parallel_safe_allows_pure_readers_only() {
    let reader = EffectProfile {
        reads_filesystem: true,
        writes_filesystem: false,
        network_access: false,
        side_effects: false,
    };
    let network_reader = EffectProfile {
        reads_filesystem: false,
        writes_filesystem: false,
        network_access: true,
        side_effects: false,
    };
    assert!(is_parallel_safe(&reader));
    assert!(is_parallel_safe(&network_reader));
}

#[test]
fn is_parallel_safe_serializes_writers_side_effects_and_interactive() {
    let writer = EffectProfile {
        reads_filesystem: true,
        writes_filesystem: true,
        network_access: false,
        side_effects: false,
    };
    let side_effecting = EffectProfile {
        reads_filesystem: true,
        writes_filesystem: true,
        network_access: false,
        side_effects: true,
    };
    let stateful = EffectProfile {
        reads_filesystem: false,
        writes_filesystem: false,
        network_access: false,
        side_effects: true,
    };
    let interactive = EffectProfile::default();
    assert!(!is_parallel_safe(&writer));
    assert!(!is_parallel_safe(&side_effecting));
    assert!(!is_parallel_safe(&stateful));
    assert!(
        !is_parallel_safe(&interactive),
        "an interactive prompt declares no read/network and must serialize"
    );
}

#[tokio::test]
async fn conversational_run_with_visible_tools_completes_in_one_turn() {
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(StreamingTestProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime(provider);
    let input = AgentLoopInput::new("explain how this code works")
        .with_agent_id(AgentId("test-agent".to_string()))
        .with_visible_tools(dummy_visible_tools())
        .with_runtime_view(Arc::new(NoopRuntimeView::new()));
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

    let outcome = run_agent_loop(&runtime, &mut loop_state, input)
        .await
        .expect("conversational loop should complete");

    assert!(matches!(
        outcome,
        LoopRunResult::Complete(AgentOutcome::Complete { .. })
    ));
    assert!(
        !loop_state.tool_executed,
        "no tool ran, so the run is conversational"
    );
    assert_eq!(loop_state.turn_count, 1);
}

struct AlwaysSucceedsExecutor {
    spec: Arc<VisibleToolSpec>,
}

#[async_trait]
impl ToolExecutor for AlwaysSucceedsExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success {
                output: format!("ran {}", call.call_id),
            },
        })
    }
}

struct SingleToolRegistry {
    spec: Arc<VisibleToolSpec>,
    executor: Arc<dyn ToolExecutor>,
}

impl SingleToolRegistry {
    fn new() -> Self {
        let spec = Arc::new(VisibleToolSpec {
            id: ToolId("tool.peek".to_string()),
            name: ToolName("peek".to_string()),
            description: "Read-only peek".to_string(),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({"type": "object"}),
            },
            output_contract: OutputContract {
                description: "peeked".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: false,
                network_access: false,
                side_effects: false,
            },
        });
        let executor: Arc<dyn ToolExecutor> = Arc::new(AlwaysSucceedsExecutor {
            spec: Arc::clone(&spec),
        });
        Self { spec, executor }
    }

    fn visible(&self) -> Vec<Arc<dyn ToolSpecView>> {
        vec![Arc::clone(&self.spec) as Arc<dyn ToolSpecView>]
    }
}

impl ToolRegistry for SingleToolRegistry {
    fn get_executor(&self, id: &ToolId) -> Option<Arc<dyn ToolExecutor>> {
        (id == self.spec.id()).then(|| Arc::clone(&self.executor))
    }

    fn get_spec(&self, id: &ToolId) -> Option<&dyn ToolSpecView> {
        (id == self.spec.id()).then(|| self.spec.as_ref() as &dyn ToolSpecView)
    }

    fn list_specs(&self) -> Vec<&dyn ToolSpecView> {
        vec![self.spec.as_ref()]
    }

    fn filter_for(&self, _agent_id: &AgentId) -> Box<dyn ToolFilter> {
        tool_filter_from_specs(&self.visible(), self)
    }
}

struct TwoToolCallProvider {
    capabilities: ProviderCapabilities,
    calls: Arc<StdMutex<usize>>,
}

impl TwoToolCallProvider {
    fn new() -> Self {
        Self {
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: true,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "two-tool-call-test".to_string(),
            },
            calls: Arc::new(StdMutex::new(0)),
        }
    }
}

#[async_trait]
impl LlmProvider for TwoToolCallProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        panic!("streaming path should use complete_stream instead of complete");
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let call_number = {
            let mut calls = self.calls.lock().expect("call counter mutex poisoned");
            *calls += 1;
            *calls
        };

        if call_number == 1 {
            return Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some("calling tools".to_string()),
                    reasoning_content: None,
                    tool_calls: vec![
                        ToolUseBlock {
                            call_id: "call_a".to_string(),
                            tool_name: "peek".to_string(),
                            input: serde_json::json!({}),
                        },
                        ToolUseBlock {
                            call_id: "call_b".to_string(),
                            tool_name: "peek".to_string(),
                            input: serde_json::json!({}),
                        },
                    ],
                    usage: Usage {
                        cached_tokens: 0,
                        prompt_tokens: 5,
                        completion_tokens: 3,
                        total_tokens: 8,
                    },
                    stop_reason: StopReason::ToolUse,
                },
                kv_cache_chunk_hashes: vec![],
            });
        }

        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some("done".to_string()),
                reasoning_content: None,
                tool_calls: Vec::new(),
                usage: Usage {
                    cached_tokens: 0,
                    prompt_tokens: 5,
                    completion_tokens: 1,
                    total_tokens: 6,
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

#[tokio::test]
async fn mid_batch_stop_still_records_every_executed_result() {
    let registry = Arc::new(SingleToolRegistry::new());
    let visible = registry.visible();
    let provider = Arc::new(LlmProviderWrapper::new(
        Arc::new(TwoToolCallProvider::new()),
        None,
        None,
    ));
    let runtime = test_runtime_with_registry(provider, 4, registry);
    let input = AgentLoopInput::new("go")
        .with_agent_id(AgentId("test-agent".to_string()))
        .with_visible_tools(visible)
        .with_runtime_view(Arc::new(NoopRuntimeView::new()))
        .with_stop_rules([LoopStopRule::AfterSuccessfulTool {
            tool_name: "peek".to_string(),
        }]);
    let mut loop_state = LoopState::new(uuid::Uuid::new_v4().to_string());

    let outcome = run_agent_loop(&runtime, &mut loop_state, input)
        .await
        .expect("loop should complete via the stop rule");

    assert!(matches!(
        outcome,
        LoopRunResult::Complete(AgentOutcome::Complete { .. })
    ));
    assert_eq!(loop_state.turn_count, 1);

    let messages = loop_state.messages.read();
    let tool_use_ids: Vec<String> = messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolUse { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let tool_result_ids: Vec<String> = messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        tool_use_ids,
        vec!["call_a".to_string(), "call_b".to_string()],
        "both tool calls should be in history"
    );
    assert_eq!(
        tool_result_ids,
        vec!["call_a".to_string(), "call_b".to_string()],
        "every executed tool_use must keep its paired tool_result even when an \
             earlier call in the batch triggered the stop rule"
    );
}
