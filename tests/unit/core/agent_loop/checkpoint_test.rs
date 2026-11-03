/// Restore while a tool-using loop is running, then inspect the actual request
/// received by the provider (not just the stored message vector).
#[tokio::test]
async fn checkpoint_restore_rebuilds_next_llm_request_during_same_run() {
    use crate::checkpoint::{
        create, CheckpointBackend, RestoreMode, RestoreRequest, SnapshotContext, SnapshotId,
    };
    struct Provider {
        capabilities: ProviderCapabilities,
        backend: Arc<dyn CheckpointBackend>,
        snapshot_id: SnapshotId,
        context: SnapshotContext,
        requests: Arc<StdMutex<Vec<LlmRequest>>>,
    }
    #[async_trait]
    impl LlmProvider for Provider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            request: &LlmRequest,
            _: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            if call == 2 {
                self.backend
                    .restore(
                        self.context.clone(),
                        RestoreRequest {
                            mode: RestoreMode::EnsureSafeMode,
                            snapshot_id: self.snapshot_id.clone(),
                        },
                    )
                    .await
                    .unwrap();
            }
            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some(format!("answer {call}")),
                    reasoning_content: None,
                    tool_calls: if call < 3 {
                        vec![ToolUseBlock {
                            call_id: format!("tool-{call}"),
                            tool_name: "bash".into(),
                            input: serde_json::json!({"command":"date"}),
                        }]
                    } else {
                        Vec::new()
                    },
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                        ..Default::default()
                    },
                    stop_reason: if call < 3 {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    },
                },
                kv_cache_chunk_hashes: Vec::new(),
            })
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }
    let backend = create(Default::default()).unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    let context = SnapshotContext {
        session_id: session_id.clone(),
    };
    let binding = backend
        .init(
            SnapshotContext {
                session_id: session_id.clone(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let snapshot = backend.save(context.clone()).unwrap();
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let runtime = test_runtime(Arc::new(LlmProviderWrapper::new(
        Arc::new(Provider {
            capabilities: EmptyCallIdToolCallProvider::new().capabilities,
            backend,
            snapshot_id: snapshot.id,
            context: context.clone(),
            requests: requests.clone(),
        }),
        None,
        None,
    )));
    let mut input = AgentLoopInput::new("original task")
        .with_agent_id(AgentId("checkpoint-test".into()))
        .with_visible_tools(dummy_visible_tools())
        .with_runtime_view(Arc::new(NoopRuntimeView::new()));
    input.checkpoint_driver = Some(Arc::new(binding.driver.unwrap()));
    let mut state = LoopState::new(session_id);
    run_agent_loop(&runtime, &mut state, input).await.unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].messages.len() > requests[0].messages.len());
    assert_eq!(
        serde_json::to_value(&requests[0].messages).unwrap(),
        serde_json::to_value(&requests[2].messages).unwrap()
    );
    assert_eq!(state.turn_count, 1, "restore also restores loop accounting");
    assert_eq!(state.token_usage.total_tokens, 15);
    assert!(!serde_json::to_string(&*state.messages.read())
        .unwrap()
        .contains("answer 2"));
}

/// A checkpoint hook must not move user-input draining before on_turn_start,
/// gate it on initialization, or consume new input only to erase it on restore.
#[tokio::test]
async fn checkpoint_preserves_pending_user_message_order_in_every_driver_state() {
    use crate::{
        checkpoint::{create, CheckpointDriver, RestoreMode, RestoreRequest, SnapshotContext},
        input::PendingUserMessageSource,
    };
    use std::collections::VecDeque;
    struct Pending {
        messages: StdMutex<VecDeque<String>>,
        events: StdMutex<Vec<&'static str>>,
    }
    #[async_trait]
    impl PendingUserMessageSource for Pending {
        async fn drain_pending_user_messages(&self) -> Vec<String> {
            self.events.lock().unwrap().push("drain");
            self.messages.lock().unwrap().drain(..).collect()
        }
    }
    impl LoopEventSink for Pending {
        fn on_turn_start(&self, _: &AgentId, _: u32) {
            self.events.lock().unwrap().push("turn_start");
            self.messages
                .lock()
                .unwrap()
                .push_back("input at turn start".into());
        }
        fn on_assistant_message(&self, _: &AgentId, _: &str) {}
        fn on_tool_result(&self, _: &AgentId, _: &ToolResultEvent) {}
        fn on_loop_end(&self, _: &AgentId, _: &LoopEndSummary) {}
    }
    struct Provider {
        healthy: StreamingTestProvider,
        pending: Arc<Pending>,
        requests: Arc<StdMutex<Vec<LlmRequest>>>,
    }
    #[async_trait]
    impl LlmProvider for Provider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            self.pending.events.lock().unwrap().push("llm");
            self.requests.lock().unwrap().push(request.clone());
            self.healthy.complete_stream(request, on_chunk).await
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            self.healthy.capabilities()
        }
    }
    for case in ["absent", "uninitialized", "active", "released", "restoring"] {
        let pending = Arc::new(Pending {
            messages: StdMutex::new(VecDeque::from(["input before boundary".into()])),
            events: StdMutex::new(Vec::new()),
        });
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let runtime = test_runtime(Arc::new(LlmProviderWrapper::new(
            Arc::new(Provider {
                healthy: StreamingTestProvider::new(),
                pending: pending.clone(),
                requests: requests.clone(),
            }),
            None,
            None,
        )));
        let mut state = LoopState::new(uuid::Uuid::new_v4().to_string());
        let context = SnapshotContext {
            session_id: state.session_id.clone(),
        };
        let backend = create(Default::default()).unwrap();
        let driver = match case {
            "absent" => None,
            "uninitialized" => Some(Arc::new(CheckpointDriver::default())),
            _ => Some(Arc::new(
                backend
                    .init(
                        SnapshotContext {
                            session_id: state.session_id.clone(),
                        },
                        serde_json::json!({}),
                    )
                    .await
                    .unwrap()
                    .driver
                    .unwrap(),
            )),
        };
        if case == "restoring" {
            state
                .messages
                .write()
                .push(ChatMessage::text(MessageRole::User, "saved task", 0));
            let snapshot = backend.save(context.clone()).unwrap();
            driver
                .as_ref()
                .unwrap()
                .process_pending(&mut state, &mut 1, false)
                .await;
            state.messages.write().push(ChatMessage::text(
                MessageRole::Assistant,
                "abandoned answer",
                0,
            ));
            backend
                .restore(
                    context.clone(),
                    RestoreRequest {
                        snapshot_id: snapshot.id,
                        mode: RestoreMode::EnsureSafeMode,
                    },
                )
                .await
                .unwrap();
        }
        if case == "released" {
            backend.release(context.clone());
        }
        let mut input = AgentLoopInput::new("task")
            .with_event_sink(pending.clone())
            .with_pending_user_messages(pending.clone());
        input.checkpoint_driver = driver;
        run_agent_loop(&runtime, &mut state, input).await.unwrap();
        assert_eq!(
            *pending.events.lock().unwrap(),
            ["turn_start", "drain", "llm"],
            "case: {case}"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let texts: Vec<_> = requests[0]
            .messages
            .iter()
            .filter_map(ChatMessage::text_content)
            .collect();
        assert!(texts.contains(&"input before boundary"), "case: {case}");
        assert!(texts.contains(&"input at turn start"), "case: {case}");
        assert!(!texts.contains(&"abandoned answer"));
    }
}

#[tokio::test]
async fn checkpoint_exit_flush_handles_complete_cancel_error_and_restore() {
    use crate::checkpoint::{
        create, CheckpointBackend, CheckpointConfig, RestoreMode, RestoreRequest, SaveMode,
        SnapshotContext, SnapshotId,
    };
    struct Provider {
        healthy: StreamingTestProvider,
        backend: Arc<dyn CheckpointBackend>,
        context: SnapshotContext,
        saved: Arc<StdMutex<Option<SnapshotId>>>,
        restore: Option<SnapshotId>,
        cancel: tokio_util::sync::CancellationToken,
        exit: &'static str,
    }
    #[async_trait]
    impl LlmProvider for Provider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            if let Some(id) = &self.restore {
                self.backend
                    .restore(
                        self.context.clone(),
                        RestoreRequest {
                            snapshot_id: id.clone(),
                            mode: RestoreMode::EnsureSafeMode,
                        },
                    )
                    .await
                    .unwrap();
            }
            *self.saved.lock().unwrap() = Some(self.backend.save(self.context.clone()).unwrap().id);
            if self.exit == "error" {
                return Err(LlmError::AuthError {
                    message: "exit-flush-test".into(),
                });
            }
            let response = self.healthy.complete_stream(request, on_chunk).await;
            if self.exit == "cancel" {
                self.cancel.cancel();
            }
            response
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            self.healthy.capabilities()
        }
    }
    for (exit, save_mode) in ["complete", "cancel", "error", "restore"]
        .into_iter()
        .flat_map(|exit| {
            [SaveMode::Next, SaveMode::Current]
                .into_iter()
                .map(move |mode| (exit, mode))
        })
    {
        let context = SnapshotContext {
            session_id: uuid::Uuid::new_v4().to_string(),
        };
        let backend = create(CheckpointConfig {
            save_mode,
            ..Default::default()
        })
        .unwrap();
        let driver = Arc::new(
            backend
                .init(context.clone(), serde_json::json!({}))
                .await
                .unwrap()
                .driver
                .unwrap(),
        );
        let mut state = LoopState::new(context.session_id.clone());
        let saved = Arc::new(StdMutex::new(None));
        if exit == "restore" && save_mode == SaveMode::Current {
            driver
                .process_pending(&mut state, &mut 1, false)
                .await;
        }
        let restore = (exit == "restore").then(|| backend.save(context.clone()).unwrap().id);
        let runtime = test_runtime(Arc::new(LlmProviderWrapper::new(
            Arc::new(Provider {
                healthy: StreamingTestProvider::new(),
                backend: backend.clone(),
                context: context.clone(),
                saved: saved.clone(),
                restore,
                cancel: state.cancel.clone(),
                exit,
            }),
            None,
            None,
        )));
        let mut input =
            AgentLoopInput::new("exit flush task").with_visible_tools(dummy_visible_tools());
        input.checkpoint_driver = Some(driver.clone());
        let result = run_agent_loop(&runtime, &mut state, input).await;
        if exit == "error" {
            assert!(result
                .err()
                .unwrap()
                .to_string()
                .contains("exit-flush-test"));
        } else {
            let outcome = match result.unwrap() {
                crate::LoopRunResult::Complete(outcome) => outcome,
                _ => panic!("unexpected suspension"),
            };
            if exit == "cancel" {
                assert!(matches!(&outcome, AgentOutcome::Cancelled { .. }));
            }
            if let AgentOutcome::Complete { messages, .. } = outcome {
                assert_eq!(messages, *state.messages.read());
            }
        }
        if exit == "restore" {
            assert!(!serde_json::to_string(&*state.messages.read())
                .unwrap()
                .contains("Hello world"));
        }
        let expected = state.messages.read().clone();
        let idle_saved = backend.save(context.clone()).unwrap();
        // EnsureSafeMode proves the command accepted during the final LLM call
        // was physically saved without another user input or loop iteration.
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    snapshot_id: saved.lock().unwrap().clone().unwrap(),
                    mode: RestoreMode::EnsureSafeMode,
                },
            )
            .await
            .unwrap();
        state.messages.write().clear();
        driver.flush(&mut state, &mut 1).await;
        if save_mode == SaveMode::Next {
            assert_eq!(*state.messages.read(), expected);
        } else {
            assert_eq!(state.messages.read().len(), 1);
            assert_eq!(
                state.messages.read()[0].text_content(),
                Some("exit flush task")
            );
            // Idle save pinned the published exit state, not the later state.
            backend
                .restore(
                    context,
                    RestoreRequest {
                        snapshot_id: idle_saved.id,
                        mode: RestoreMode::EnsureSafeMode,
                    },
                )
                .await
                .unwrap();
            driver.flush(&mut state, &mut 1).await;
            assert_eq!(*state.messages.read(), expected);
        }
    }
}

#[tokio::test]
async fn checkpoint_idle_restore_preserves_new_input_or_replays_final_step() {
    use crate::checkpoint::{
        create, CheckpointBackend, CheckpointConfig, RestoreMode, RestoreRequest, SaveMode,
        SnapshotContext, SnapshotId,
    };
    struct ToolPromptBuilder;
    #[async_trait]
    impl PromptBuilder for ToolPromptBuilder {
        async fn build(&self, input: PromptBuildInput) -> Result<PromptBuildResult, PromptBuildError> {
            let tools = input.visible_tools.iter().map(|tool| agent_types::Tool {
                name: tool.name().0.clone(),
                description: tool.description().to_string(),
                parameters: tool.input_schema().schema.clone(),
            }).collect();
            let mut result = FixedPromptBuilder.build(input).await?;
            result.request.tools = tools;
            Ok(result)
        }
    }
    struct Provider {
        healthy: StreamingTestProvider,
        backend: Arc<dyn CheckpointBackend>,
        context: SnapshotContext,
        snapshot_id: SnapshotId,
        running: bool,
        requests: Arc<StdMutex<Vec<LlmRequest>>>,
    }
    #[async_trait]
    impl LlmProvider for Provider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            if self.running && call == 1 {
                self.backend
                    .restore(
                        self.context.clone(),
                        RestoreRequest {
                            snapshot_id: self.snapshot_id.clone(),
                            mode: RestoreMode::EnsureSafeMode,
                        },
                    )
                    .await
                    .unwrap();
                return Ok(LlmResponse {
                    message: AssistantMessage {
                        text: None,
                        reasoning_content: Some("new tool reasoning fixture".into()),
                        tool_calls: vec![ToolUseBlock {
                            call_id: "discarded_call".into(),
                            tool_name: "bash".into(),
                            input: serde_json::json!({"command":"pwd"}),
                        }],
                        usage: Usage::default(),
                        stop_reason: StopReason::ToolUse,
                    },
                    kv_cache_chunk_hashes: Vec::new(),
                });
            }
            self.healthy.complete_stream(request, on_chunk).await
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            self.healthy.capabilities()
        }
    }
    for mode in [SaveMode::Next, SaveMode::Current] {
        for running in [false, true] {
            let backend = create(CheckpointConfig {
                save_mode: mode,
                ..Default::default()
            })
            .unwrap();
            let context = SnapshotContext {
                session_id: uuid::Uuid::new_v4().to_string(),
            };
            let driver = Arc::new(
                backend
                    .init(context.clone(), serde_json::json!({}))
                    .await
                    .unwrap()
                    .driver
                    .unwrap(),
            );
            let mut state = LoopState::new(context.session_id.clone());
            let mut call = ChatMessage::new(
                MessageRole::Assistant,
                vec![ContentBlock::ToolUse {
                    call_id: "saved_call".into(),
                    tool_name: "bash".into(),
                    input: serde_json::json!({"command":"pwd"}),
                }],
                None,
                0,
                None,
            );
            call.reasoning_content = Some("saved tool reasoning fixture".into());
            let mut answer = ChatMessage::text(MessageRole::Assistant, "A final response", 0);
            answer.reasoning_content = Some("saved response reasoning fixture".into());
            let saved_history = vec![
                ChatMessage::user("A task"),
                call,
                ChatMessage::tool_result("saved_call", "bash", "saved output", false, 0),
                answer,
            ];
            *state.messages.write() = saved_history.clone();
            driver.flush(&mut state, &mut 1).await;
            let snapshot = backend.save(context.clone()).unwrap();
            driver.flush(&mut state, &mut 1).await;
            state.messages.write().push(ChatMessage::user("B task"));
            if !running {
                backend
                    .restore(
                        context.clone(),
                        RestoreRequest {
                            snapshot_id: snapshot.id.clone(),
                            mode: RestoreMode::EnsureSafeMode,
                        },
                    )
                    .await
                    .unwrap();
            }
            let requests = Arc::new(StdMutex::new(Vec::new()));
            let runtime = test_runtime(Arc::new(LlmProviderWrapper::new(
                Arc::new(Provider {
                    healthy: StreamingTestProvider {
                        capabilities: EmptyCallIdToolCallProvider::new().capabilities,
                    },
                    backend: backend.clone(),
                    context: context.clone(),
                    snapshot_id: snapshot.id.clone(),
                    running,
                    requests: requests.clone(),
                }),
                None,
                None,
            )));
            runtime.set_prompt_builder(Arc::new(ToolPromptBuilder));
            let mut input =
                AgentLoopInput::new("I new input")
                    .with_agent_id(AgentId("checkpoint-idle-test".into()))
                    .with_runtime_view(Arc::new(NoopRuntimeView::new()))
                    .with_visible_tools(dummy_visible_tools());
            input.checkpoint_driver = Some(driver.clone());
            run_agent_loop(&runtime, &mut state, input).await.unwrap();
            {
                let requests = requests.lock().unwrap();
                assert_eq!(requests.len(), if running { 2 } else { 1 });
                let request = requests.last().unwrap();
                let expected_history = if running {
                    &saved_history[..3]
                } else {
                    &saved_history[..]
                };
                let history: Vec<_> = request
                    .messages
                    .iter()
                    .filter(|message| message.role != MessageRole::System)
                    .collect();
                assert_eq!(
                    &history[..expected_history.len()],
                    &expected_history.iter().collect::<Vec<_>>()
                );
                if running {
                    assert_eq!(history.len(), 3);
                    assert_eq!(history.last().unwrap().role, MessageRole::Tool);
                } else {
                    assert_eq!(history.len(), 5);
                    assert_eq!(history.last().unwrap().text_content(), Some("I new input"));
                }
                assert_eq!(request.tools.len(), 1);
                assert_eq!(request.tools[0].name, "bash");
                assert_eq!(
                    request.tools[0].parameters["properties"]["command"]["type"],
                    "string"
                );
            }
            if !running {
                // A second invocation also starts at turn 1 and must retain its own input.
                backend.restore(context.clone(), RestoreRequest {
                    snapshot_id: snapshot.id.clone(), mode: RestoreMode::EnsureSafeMode,
                }).await.unwrap();
                let mut input = AgentLoopInput::new("second input");
                input.checkpoint_driver = Some(driver.clone());
                run_agent_loop(&runtime, &mut state, input).await.unwrap();
                let requests = requests.lock().unwrap();
                let history = &requests.last().unwrap().messages;
                assert_eq!(&history[..saved_history.len()], &saved_history);
                assert_eq!(history.len(), saved_history.len() + 1);
                assert_eq!(history.last().unwrap().text_content(), Some("second input"));
            }
            // Running restores must not mutate the saved idle snapshot itself.
            backend
                .restore(
                    context,
                    RestoreRequest {
                        snapshot_id: snapshot.id,
                        mode: RestoreMode::EnsureSafeMode,
                    },
                )
                .await
                .unwrap();
            driver.flush(&mut state, &mut 1).await;
            assert_eq!(*state.messages.read(), saved_history);
        }
    }
}

#[tokio::test]
async fn checkpoint_error_does_not_end_agent_task() {
    use crate::checkpoint::{create, SnapshotContext};
    let backend = create(Default::default()).unwrap();
    let driver = backend
        .init(
            SnapshotContext {
                session_id: "other-session".into(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap()
        .driver
        .unwrap();
    let runtime = test_runtime(Arc::new(LlmProviderWrapper::new(
        Arc::new(StreamingTestProvider::new()), None, None,
    )));
    let mut state = LoopState::new("active-session".into());
    let mut input = AgentLoopInput::new("task");
    input.checkpoint_driver = Some(Arc::new(driver));
    // SessionMismatch is raised at the checkpoint, but the provider must still run.
    run_agent_loop(&runtime, &mut state, input).await.unwrap();
    assert_eq!(state.messages.read().last().unwrap().text_content(), Some("Hello world"));
}

#[tokio::test]
async fn checkpoint_current_saved_during_final_response_ignores_next_user_input() {
    use crate::checkpoint::{
        create, CheckpointBackend, CheckpointConfig, RestoreMode, RestoreRequest, SaveMode,
        SnapshotContext, SnapshotId,
    };
    struct Provider {
        capabilities: ProviderCapabilities,
        backend: Arc<dyn CheckpointBackend>,
        saved: Arc<StdMutex<Option<SnapshotId>>>,
        context: SnapshotContext,
        requests: Arc<StdMutex<Vec<LlmRequest>>>,
    }
    #[async_trait]
    impl LlmProvider for Provider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            request: &LlmRequest,
            _: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            let call = {
                let mut requests = self.requests.lock().unwrap();
                requests.push(request.clone());
                requests.len()
            };
            if call == 2 {
                *self.saved.lock().unwrap() =
                    Some(self.backend.save(self.context.clone()).unwrap().id);
            }
            Ok(LlmResponse {
                message: AssistantMessage {
                    text: Some(format!("answer {call}")),
                    reasoning_content: Some(format!("reasoning {call}")),
                    tool_calls: if call == 1 {
                        vec![ToolUseBlock {
                            call_id: format!("tool-{call}"),
                            tool_name: "bash".into(),
                            input: serde_json::json!({"command":"date"}),
                        }]
                    } else {
                        Vec::new()
                    },
                    usage: Usage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                        total_tokens: 15,
                        ..Default::default()
                    },
                    stop_reason: if call == 1 {
                        StopReason::ToolUse
                    } else {
                        StopReason::EndTurn
                    },
                },
                kv_cache_chunk_hashes: Vec::new(),
            })
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }
    let backend = create(CheckpointConfig {
        save_mode: SaveMode::Current,
        ..Default::default()
    })
    .unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    let context = SnapshotContext {
        session_id: session_id.clone(),
    };
    let binding = backend
        .init(
            SnapshotContext {
                session_id: session_id.clone(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let saved = Arc::new(StdMutex::new(None));
    let requests = Arc::new(StdMutex::new(Vec::new()));
    let runtime = test_runtime(Arc::new(LlmProviderWrapper::new(
        Arc::new(Provider {
            capabilities: EmptyCallIdToolCallProvider::new().capabilities,
            backend: backend.clone(),
            saved: saved.clone(),
            context: context.clone(),
            requests: requests.clone(),
        }),
        None,
        None,
    )));
    let driver = Arc::new(binding.driver.unwrap());
    let make_input = |text| {
        let mut input = AgentLoopInput::new(text)
            .with_agent_id(AgentId("checkpoint-test".into()))
            .with_visible_tools(dummy_visible_tools())
            .with_runtime_view(Arc::new(NoopRuntimeView::new()));
        input.checkpoint_driver = Some(driver.clone());
        input
    };
    let mut state = LoopState::new(session_id);
    run_agent_loop(&runtime, &mut state, make_input("original task"))
        .await
        .unwrap();
    let snapshot_id = saved.lock().unwrap().clone().unwrap();
    backend
        .restore(
            context,
            RestoreRequest {
                snapshot_id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    run_agent_loop(
        &runtime,
        &mut state,
        make_input("trigger input must be ignored"),
    )
    .await
    .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        serde_json::to_value(&requests[1]).unwrap(),
        serde_json::to_value(&requests[2]).unwrap()
    );
    assert_eq!(requests[2].messages.last().unwrap().role, MessageRole::Tool);
    assert!(!serde_json::to_string(&requests[2])
        .unwrap()
        .contains("trigger input must be ignored"));
}

#[tokio::test]
async fn checkpoint_exit_restore_returns_reply_from_restored_history() {
    use crate::checkpoint::{
        create, CheckpointBackend, RestoreMode, RestoreRequest, SnapshotContext, SnapshotId,
    };
    struct Provider {
        healthy: StreamingTestProvider,
        backend: Arc<dyn CheckpointBackend>,
        context: SnapshotContext,
        snapshot: SnapshotId,
        cancel: Option<tokio_util::sync::CancellationToken>,
    }
    #[async_trait]
    impl LlmProvider for Provider {
        async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unreachable!()
        }
        async fn complete_stream(
            &self,
            request: &LlmRequest,
            on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            self.backend
                .restore(
                    self.context.clone(),
                    RestoreRequest {
                        snapshot_id: self.snapshot.clone(),
                        mode: RestoreMode::EnsureSafeMode,
                    },
                )
                .await
                .unwrap();
            let response = self.healthy.complete_stream(request, on_chunk).await;
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            }
            response
        }
        fn capabilities(&self) -> &ProviderCapabilities {
            self.healthy.capabilities()
        }
    }
    for exit in ["complete", "max_turns", "cancelled"] {
        for saved_answer in [None, Some("saved answer")] {
            let backend = create(Default::default()).unwrap();
            let context = SnapshotContext {
                session_id: uuid::Uuid::new_v4().to_string(),
            };
            let driver = Arc::new(
                backend
                    .init(context.clone(), serde_json::json!({}))
                    .await
                    .unwrap()
                    .driver
                    .unwrap(),
            );
            let mut state = LoopState::new(context.session_id.clone());
            state.messages.write().push(ChatMessage::user("saved task"));
            if let Some(answer) = saved_answer {
                state
                    .messages
                    .write()
                    .push(ChatMessage::text(MessageRole::Assistant, answer, 0));
            }
            let saved_history = state.messages.read().clone();
            let snapshot = backend.save(context.clone()).unwrap();
            driver.flush(&mut state, &mut 1).await;
            let runtime = test_runtime_with_max_turns(
                Arc::new(LlmProviderWrapper::new(
                    Arc::new(Provider {
                        healthy: StreamingTestProvider::new(),
                        backend,
                        context,
                        snapshot: snapshot.id,
                        cancel: (exit == "cancelled").then(|| state.cancel.clone()),
                    }),
                    None,
                    None,
                )),
                if exit == "max_turns" { 1 } else { 10 },
            );
            let mut input = AgentLoopInput::new("discarded branch");
            input.checkpoint_driver = Some(driver);
            let LoopRunResult::Complete(outcome) =
                run_agent_loop(&runtime, &mut state, input).await.unwrap()
            else {
                panic!("unexpected suspension")
            };
            let (reply, messages, turn_count, estimated) = match (exit, outcome) {
                (
                    "complete",
                    AgentOutcome::Complete {
                        reply,
                        messages,
                        turn_count,
                        estimated_input_tokens,
                        ..
                    },
                ) => (Some(reply), messages, turn_count, estimated_input_tokens),
                (
                    "max_turns",
                    AgentOutcome::MaxTurnsReached {
                        partial_reply,
                        messages,
                        turn_count,
                        estimated_input_tokens,
                        ..
                    },
                )
                | (
                    "cancelled",
                    AgentOutcome::Cancelled {
                        partial_reply,
                        messages,
                        turn_count,
                        estimated_input_tokens,
                        ..
                    },
                ) => (partial_reply, messages, turn_count, estimated_input_tokens),
                _ => panic!("unexpected outcome for {exit}"),
            };
            let expected = if exit == "complete" {
                Some(saved_answer.unwrap_or_default())
            } else {
                saved_answer
            };
            assert_eq!(reply.as_deref(), expected, "{exit}");
            assert_eq!(messages, saved_history);
            assert_eq!(turn_count, 0);
            assert_eq!(estimated, 0);
        }
    }
}
