use super::*;

#[tokio::test]
async fn controlled_open_requires_a_bound_backend() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let credential = temp.path().join("key");
    std::fs::write(&credential, "test-key").unwrap();
    let profile = ControlledModelProfile {
        provider: "openai".to_string(),
        model: "test-model".to_string(),
        api_base: Some("http://127.0.0.1:1/v1/chat/completions".to_string()),
        api_key_env: None,
        api_key_file: Some(credential),
        context_window: None,
    };

    let error = ControlledTextSession::open_with_backend_manager(
        "unbound".to_string(),
        workspace.clone(),
        &profile,
        Arc::new(BackendManager::new_controlled()),
    )
    .await
    .err()
    .expect("a controlled session cannot open without a binding");

    assert!(matches!(
        error.downcast_ref::<crate::gateway::SessionServiceError>(),
        Some(crate::gateway::SessionServiceError::RuntimeBuild { message })
            if message == "controlled session has no operation backend binding"
    ));
    assert_eq!(std::fs::read_dir(workspace).unwrap().count(), 0);
}

struct TimesOutThenReplies {
    attempts: std::sync::atomic::AtomicUsize,
    capabilities: ProviderCapabilities,
}

#[async_trait]
impl LlmProvider for TimesOutThenReplies {
    async fn complete(&self, _: &LlmRequest) -> Result<LlmResponse, LlmError> {
        use agent_types::llm::{AssistantMessage, StopReason, Usage};

        if self
            .attempts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            std::future::pending::<()>().await;
        }
        Ok(LlmResponse {
            message: AssistantMessage {
                text: Some("done".into()),
                reasoning_content: None,
                tool_calls: Vec::new(),
                usage: Usage {
                    prompt_tokens: 4,
                    completion_tokens: 2,
                    total_tokens: 6,
                    ..Default::default()
                },
                stop_reason: StopReason::EndTurn,
            },
            kv_cache_chunk_hashes: Vec::new(),
        })
    }

    async fn complete_stream(
        &self,
        _: &LlmRequest,
        _: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        unreachable!("the completion retry uses the non-streaming provider")
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_attempt_leaves_successful_retry_usage_partial() {
    use std::sync::atomic::Ordering;

    let provider = Arc::new(TimesOutThenReplies {
        attempts: Default::default(),
        capabilities: ProviderCapabilities {
            supports_streaming: false,
            supports_tool_calls: false,
            supports_json_mode: false,
            max_context_window: 4096,
            model_name: "test-model".into(),
        },
    });
    let usage = Arc::new(Mutex::new(TurnUsage::default()));
    let metered = Arc::new(MeteredProvider {
        inner: provider.clone(),
        usage: usage.clone(),
    });
    let wrapper = LlmProviderWrapper::new(metered, None, None);
    let request = LlmRequest {
        messages: vec![],
        tools: vec![],
        tool_choice: Default::default(),
        max_tokens: None,
        temperature: None,
        response_format: Default::default(),
        reasoning_effort: Default::default(),
    };

    wrapper
        .complete_with_budget(&request, llm_client::RetryBudget::Background)
        .await
        .expect("the second attempt succeeds after the wrapper deadline drops the first");
    assert_eq!(provider.attempts.load(Ordering::Relaxed), 2);

    let report = usage.lock().unwrap().report(false);
    assert_eq!(report.status, "partial");
    assert_eq!(
        (report.input_tokens, report.output_tokens),
        (Some(4), Some(2))
    );
}
