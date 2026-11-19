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

#[tokio::test]
async fn duplicate_controlled_binding_preserves_the_original_lease() {
    let manager = BackendManager::new_controlled();
    let session_id = "controlled";
    let binding = |id: &str| {
        let root = BackendPath::from_raw("/client/workspace");
        let instance = BackendInstance {
            backend_id: BackendId(id.into()),
            provider: BackendProviderKind("acp".into()),
            instance_id: BackendInstanceId(id.into()),
            session_id: session_id.into(),
            state: BackendLifecycleState::Active,
            workspace_root: root.clone(),
            endpoint: None,
            snapshot: None,
            capabilities: Default::default(),
            resources: Default::default(),
            metadata: serde_json::Value::Null,
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let backend = Arc::new(TextOnlyBackend {
            session_id: id.into(),
            root,
        });
        (instance, backend)
    };
    let (original, backend) = binding("original");
    manager.bind_controlled(original, backend).await.unwrap();
    let lease = manager.lease_bound_session(session_id).await.unwrap();

    let (replacement, backend) = binding("replacement");
    let error = manager
        .bind_controlled(replacement, backend)
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::backend::BackendError::Conflict { message }
            if message == "controlled session controlled already has an operation backend binding"
    ));
    let current = manager.lease_bound_session(session_id).await.unwrap();
    assert_eq!(current.instance().backend_id.0, "original");
    assert!(Arc::ptr_eq(&lease.backend(), &current.backend()));
}
