use super::*;
use agent_llm::{ChatMessageExt, LlmRequestExt};

fn make_provider() -> OpenAiFamilyProvider {
    OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.openai.com/v1".to_string(),
        "gpt-5.4".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    )
}

#[test]
fn build_body_sets_reasoning_effort() {
    let provider = make_provider();
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Max);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["reasoning_effort"], "xhigh");
}

#[test]
fn build_body_omits_reasoning_effort_when_default() {
    let provider = make_provider();
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Default);

    let body = provider.build_body(&request, false).unwrap();

    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn build_body_omits_reasoning_effort_when_off() {
    let provider = make_provider();
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Off);

    let body = provider.build_body(&request, false).unwrap();

    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn gpt_5_6_with_tools_sets_none_reasoning_effort_when_off() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.openai.com/v1".to_string(),
        "gpt-5.6-sol".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Off)
        .with_tools(vec![agent_types::Tool {
            name: "lookup".to_string(),
            description: "Look something up".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }]);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["reasoning_effort"], "none");
}

#[test]
fn gpt_5_6_without_tools_sets_none_reasoning_effort_when_off() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.openai.com/v1".to_string(),
        "openai/gpt-5.6-terra".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Off);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["reasoning_effort"], "none");
}

#[test]
fn gpt_5_6_default_omits_reasoning_effort() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.openai.com/v1".to_string(),
        "gpt-5.6-sol".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Default);

    let body = provider.build_body(&request, false).unwrap();

    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn kimi_k3_maps_supported_reasoning_effort_values() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.moonshot.cn/v1".to_string(),
        "kimi-k3".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Max);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["reasoning_effort"], "max");
}

#[test]
fn glm_5_2_disables_thinking_when_reasoning_is_off() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://open.bigmodel.cn/api/paas/v4".to_string(),
        "glm-5.2".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Off);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["thinking"]["type"], "disabled");
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn glm_5_2_default_omits_thinking_and_reasoning_effort() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://open.bigmodel.cn/api/paas/v4".to_string(),
        "glm-5.2".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Default);

    let body = provider.build_body(&request, false).unwrap();

    assert!(body.get("thinking").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn glm_5_2_maps_max_reasoning_effort_to_max() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://open.bigmodel.cn/api/paas/v4".to_string(),
        "z-ai/glm-5.2".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Max);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["reasoning_effort"], "max");
    assert!(body.get("thinking").is_none());
}

// Regression test for the URL fallback loop: when *every* candidate URL
// returns HTTP 401, the provider must surface the real "Unauthorized"
// error to the caller instead of the wrapped "All N candidates failed"
// summary. (Previously the surfacing logic lived in dead post-loop code
// that was never reached, so callers saw the wrapped message.)
#[tokio::test]
async fn complete_surfaces_real_401_when_all_candidates_unauthorized() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", mockito::Matcher::Any)
        .with_status(401)
        .with_body(
            r#"{"error":{"message":"Missing API key","type":"authentication_error","code":401}}"#,
        )
        .create_async()
        .await;

    let provider = OpenAiFamilyProvider::new(
        "bad-key".to_string(),
        server.url(),
        "gpt-5.4".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );

    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hi")]);
    let error = provider.complete(&request).await.unwrap_err();

    assert!(
        is_http_unauthorized_error(&error),
        "expected raw HTTP 401, got: {error}"
    );
    assert!(
        !error.to_string().contains("candidates failed"),
        "should not be wrapped URL-fallback summary: {error}"
    );
    assert!(error.to_string().contains("HTTP 401"));
}

// Streaming counterpart of the regression above: `complete_stream` shares
// the same fallback loop and must also surface the real HTTP 401 when
// every candidate rejects the request, instead of the wrapped summary.
#[tokio::test]
async fn complete_stream_surfaces_real_401_when_all_candidates_unauthorized() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", mockito::Matcher::Any)
        .with_status(401)
        .with_body(
            r#"{"error":{"message":"Missing API key","type":"authentication_error","code":401}}"#,
        )
        .create_async()
        .await;

    let provider = OpenAiFamilyProvider::new(
        "bad-key".to_string(),
        server.url(),
        "gpt-5.4".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );

    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hi")]);
    let on_chunk: &(dyn Fn(StreamChunk) + Send + Sync) = &|_chunk| {};
    let error = provider
        .complete_stream(&request, on_chunk)
        .await
        .unwrap_err();

    assert!(
        is_http_unauthorized_error(&error),
        "expected raw HTTP 401, got: {error}"
    );
    assert!(
        !error.to_string().contains("candidates failed"),
        "should not be wrapped URL-fallback summary: {error}"
    );
    assert!(error.to_string().contains("HTTP 401"));
}

#[test]
fn compatible_provider_preserves_explicit_json_capability_and_default() {
    assert!(make_provider().capabilities().supports_json_mode);
    for supported in [false, true] {
        let mut capabilities = make_provider().capabilities().clone();
        capabilities.supports_json_mode = supported;
        let provider = OpenAiCompatibleProvider::new(OpenAiCompatibleProviderConfig::new(
            "http://localhost:8080/v1",
            "test-key",
            capabilities,
        ))
        .unwrap();
        assert_eq!(provider.capabilities().supports_json_mode, supported);
        let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
        assert!(provider
            .inner
            .build_body(&request, false)
            .unwrap()
            .get("response_format")
            .is_none());
    }
}

#[test]
fn stream_error_frame_preserves_gateway_diagnostics() {
    let line = r#"data: {"error":{"message":"[model_service] upstream omitted a completion reason","type":"model_error","code":"invalid_upstream_response","param":null}}"#;
    let error = parse_openai_family_stream_line(line).unwrap_err();
    match error {
        LlmError::ApiError(message) => {
            for diagnostic in [
                "[model_service] upstream omitted a completion reason",
                "model_error",
                "invalid_upstream_response",
            ] {
                assert!(message.contains(diagnostic), "{message}");
            }
            assert!(!message.contains("missing field"));
        }
        other => panic!("expected provider API error, got {other:?}"),
    }
}

#[test]
fn stream_error_frame_classifies_openai_rate_limits() {
    for fields in [
        serde_json::json!({"type":"tokens","code":"rate_limit_exceeded"}),
        serde_json::json!({"type":"rate_limit_error","code":null}),
        serde_json::json!({"type":"insufficient_quota","code":"insufficient_quota"}),
    ] {
        let mut error = fields;
        error["message"] = serde_json::json!("Rate limit reached for model");
        let body = serde_json::json!({"error":error}).to_string();
        let streamed = parse_openai_family_stream_line(&format!("data: {body}"));
        let http = map_api_status_error(reqwest::StatusCode::TOO_MANY_REQUESTS, &body, "", None);
        match (streamed, http) {
            (
                Err(LlmError::RateLimited {
                    retry_after_ms,
                    message,
                }),
                LlmError::RateLimited {
                    retry_after_ms: expected,
                    ..
                },
            ) => {
                assert_eq!(retry_after_ms, expected);
                assert!(message.contains("Rate limit reached"));
                assert!(message.contains(error["type"].as_str().unwrap()));
            }
            (actual, _) => panic!("expected rate limit, got {actual:?}"),
        }
    }
}

#[test]
fn stream_error_frame_classifies_openai_context_overflow() {
    for body in [
        r#"{"error":{"message":"too many tokens","type":"invalid_request_error","code":"context_length_exceeded","param":"messages"}}"#,
        r#"{"error":{"message":"This model's maximum context length is 8192 tokens","type":"invalid_request_error","code":null}}"#,
    ] {
        let error = parse_openai_family_stream_line(&format!("data: {body}")).unwrap_err();
        assert!(
            matches!(error, LlmError::ContextLengthExceeded { .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("invalid_request_error"));
    }
}

#[test]
fn stream_error_frame_keeps_legacy_and_normal_chunks() {
    for body in [
        r#"{"type":"error","error":{"code":"invalid_prompt","message":"bad prompt"}}"#,
        r#"{"type":"error","error":{"code":"context_length_exceeded","message":"overflow"}}"#,
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"busy"}}"#,
    ] {
        let actual = parse_openai_family_stream_line(&format!("data: {body}")).unwrap_err();
        assert_eq!(
            actual.to_string(),
            crate::error::parse_stream_error(body).unwrap().to_string()
        );
    }
    let parsed = parse_openai_family_stream_line(
        r#"data: {"id":"chatcmpl-test","object":"chat.completion.chunk","created":1,"model":"test","choices":[{"index":0,"delta":{"content":"hello","reasoning_content":"thinking"},"finish_reason":"stop"}],"error":null}"#,
    ).unwrap().unwrap();
    assert_eq!(parsed.content.as_deref(), Some("hello"));
    assert_eq!(parsed.reasoning.as_deref(), Some("thinking"));
    assert_eq!(parsed.finish_reason.as_deref(), Some("stop"));
    assert!(parse_openai_family_stream_line("data: [DONE]")
        .unwrap()
        .is_none());
    assert!(matches!(
        parse_openai_family_stream_line("data: {broken"),
        Err(LlmError::ParseError(_))
    ));
}

#[tokio::test]
async fn stream_error_frame_redacts_credentials_at_provider_boundary() {
    let mut server = mockito::Server::new_async().await;
    let body = serde_json::json!({"error":{
        "message":"[model_service] upstream omitted a completion reason; credential-test-secret",
        "type":"model_error", "code":"invalid_upstream_response"
    }});
    let mock = server
        .mock("POST", mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(format!("data: {body}\n\ndata: [DONE]\n\n"))
        .expect(1)
        .create_async()
        .await;
    let provider = OpenAiFamilyProvider::new(
        "credential-test-secret".into(),
        server.url(),
        "test".into(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let chunks = std::sync::atomic::AtomicUsize::new(0);
    let error = provider
        .complete_stream(
            &LlmRequest::new(vec![agent_types::ChatMessage::user("hi")]),
            &|_| {
                chunks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, LlmError::ApiError(_)), "{error:?}");
    let message = error.to_string();
    assert!(message.contains("upstream omitted a completion reason"));
    assert!(message.contains("model_error"));
    assert!(message.contains("invalid_upstream_response"));
    assert!(!message.contains("credential-test-secret"));
    assert_eq!(chunks.load(std::sync::atomic::Ordering::Relaxed), 0);
    mock.assert_async().await;
}

#[tokio::test(start_paused = true)]
async fn stream_error_frame_retry_policy_matches_http_errors() {
    for (body, status, retryable) in [
        (
            r#"{"error":{"message":"[model_service] upstream omitted a completion reason","type":"model_error","code":"invalid_upstream_response"}}"#,
            reqwest::StatusCode::BAD_REQUEST,
            false,
        ),
        (
            r#"{"error":{"message":"Rate limit reached","type":"tokens","code":"rate_limit_exceeded"}}"#,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            true,
        ),
        (
            r#"{"error":{"message":"too many tokens","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
            reqwest::StatusCode::BAD_REQUEST,
            false,
        ),
        (
            r#"{"error":{"message":"connection reset by peer","type":"server_error","code":null}}"#,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            true,
        ),
    ] {
        let streamed = parse_openai_family_stream_line(&format!("data: {body}")).unwrap_err();
        let http = map_api_status_error(status, body, "", None);
        for error in [streamed, http] {
            let mut calls = 0;
            let result = crate::request_policy::run_stream(|_| {
                calls += 1;
                std::future::ready(if calls == 1 {
                    Err(error.clone())
                } else {
                    Ok(())
                })
            })
            .await;
            assert_eq!(result.is_ok(), retryable, "{error:?}");
            assert_eq!(calls, if retryable { 2 } else { 1 }, "{error:?}");
        }
    }
}
