use super::*;
use agent_llm::{ChatMessageExt, LlmRequestExt};

fn make_provider() -> OpenAiFamilyProvider {
    OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.openai.com/v1".to_string(),
        "gpt-5.4".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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
        Default::default(),
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

// Regression test for the 5xx fallback path: a 5xx on one real
// `/chat/completions` route means the route is likely correct but the
// backend is temporarily unavailable, so the next real route must be tried
// instead of aborting with the raw error.
#[tokio::test]
async fn complete_tries_next_real_path_after_first_route_5xx() {
    let mut server = mockito::Server::new_async().await;
    let _root = server
        .mock("POST", "/")
        .with_status(404)
        .create_async()
        .await;
    let _v1 = server
        .mock("POST", "/v1")
        .with_status(404)
        .create_async()
        .await;
    let _v4 = server
        .mock("POST", "/v4")
        .with_status(404)
        .create_async()
        .await;
    let _first_path = server
        .mock("POST", "/chat/completions")
        .with_status(503)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":{"message":"backend unavailable"}}"#)
        .create_async()
        .await;
    let _second_path = server
        .mock("POST", "/v1/chat/completions")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"id":"x","object":"chat.completion","created":0,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
        )
        .create_async()
        .await;

    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        server.url(),
        "gpt-5.4".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        Default::default(),
    );

    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hi")]);
    let response = provider.complete(&request).await.unwrap();

    assert_eq!(response.message.text.as_deref(), Some("OK"));
}

// When every real completions route returns 5xx, the final error must be
// the raw upstream error (status code + gateway error code), not the wrapped
// "All N candidates failed" summary that hides the root cause.
#[tokio::test]
async fn complete_surfaces_raw_5xx_when_all_real_paths_fail() {
    let mut server = mockito::Server::new_async().await;
    let _root = server
        .mock("POST", "/")
        .with_status(404)
        .create_async()
        .await;
    let _v1 = server
        .mock("POST", "/v1")
        .with_status(404)
        .create_async()
        .await;
    let _v4 = server
        .mock("POST", "/v4")
        .with_status(404)
        .create_async()
        .await;
    let upstream_body = r#"data: {"error":{"message":"Upstream error (503): {\"error_msg\":\"Service abc backend is not available\",\"error_code\":\"ModelArts.8903\"}","type":"upstream_error","code":502}}data: [DONE]"#;
    let _completions = server
        .mock("POST", "/chat/completions")
        .with_status(502)
        .with_header("content-type", "application/json")
        .with_body(upstream_body)
        .create_async()
        .await;
    let _v1_completions = server
        .mock("POST", "/v1/chat/completions")
        .with_status(502)
        .with_header("content-type", "application/json")
        .with_body(upstream_body)
        .create_async()
        .await;
    let _v4_completions = server
        .mock("POST", "/v4/chat/completions")
        .with_status(502)
        .with_header("content-type", "application/json")
        .with_body(upstream_body)
        .create_async()
        .await;

    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        server.url(),
        "gpt-5.4".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        Default::default(),
    );

    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hi")]);
    let error = provider.complete(&request).await.unwrap_err();
    let message = error.to_string();

    assert!(message.contains("HTTP 502"), "expected raw 502: {message}");
    assert!(
        message.contains("ModelArts.8903"),
        "expected upstream error code: {message}"
    );
    assert!(
        !message.contains("candidates failed"),
        "should not be wrapped URL-fallback summary: {message}"
    );
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
        Default::default(),
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
    // The legacy top-level `{"type":"error",...}` envelope must keep its
    // historical classification through the production parser: only
    // `context_length_exceeded` maps to the context-overflow variant, and
    // everything else stays a plain API error — including
    // `rate_limit_error`, which only the modern `error`-object envelope
    // promotes to RateLimited.
    for (body, expect_overflow) in [
        (
            r#"{"type":"error","error":{"code":"invalid_prompt","message":"bad prompt"}}"#,
            false,
        ),
        (
            r#"{"type":"error","error":{"code":"context_length_exceeded","message":"overflow"}}"#,
            true,
        ),
        (
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"busy"}}"#,
            false,
        ),
    ] {
        let actual = parse_openai_family_stream_line(&format!("data: {body}")).unwrap_err();
        match &actual {
            LlmError::ContextLengthExceeded { .. } => {
                assert!(expect_overflow, "body: {body}");
            }
            LlmError::ApiError(_) => {
                assert!(!expect_overflow, "body: {body}");
            }
            other => panic!("unexpected classification for {body}: {other:?}"),
        }
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
        "message":"[model_service] upstream omitted a completion reason; credential-test-secret; custom-header-test-secret",
        "type":"model_error", "code":"invalid_upstream_response"
    }});
    let mock = server
        .mock("POST", mockito::Matcher::Any)
        .match_header("x-task-id", "custom-header-test-secret")
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(format!("data: {body}\n\ndata: [DONE]\n\n"))
        .expect(1)
        .create_async()
        .await;
    let mut headers = reqwest::header::HeaderMap::new();
    let mut value = reqwest::header::HeaderValue::from_static("custom-header-test-secret");
    value.set_sensitive(true);
    headers.insert("x-task-id", value);
    let provider = OpenAiFamilyProvider::new(
        "credential-test-secret".into(),
        server.url(),
        "test".into(),
        OpenAiFamilyAuthStyle::Bearer,
        headers,
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
    assert!(!message.contains("custom-header-test-secret"));
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

#[tokio::test]
async fn explicit_output_ceiling_reaches_streaming_and_background_completion_retries() {
    use mockito::Matcher;
    use serde_json::json;

    for streaming in [false, true] {
        let mut server = mockito::Server::new_async().await;
        let config = crate::LlmProviderConfig::new("openai", "token-budget-test")
            .with_api_key("loopback-key")
            .with_api_base(format!("{}/chat/completions", server.url()));
        let provider = crate::create_llm_provider(&config, None, None)
            .unwrap()
            .with_max_output_tokens(512, ReasoningEffort::Default)
            .unwrap()
            .for_agent("child".into());
        let request_body = Matcher::PartialJson(if streaming {
            json!({"max_tokens":512,"stream":true})
        } else {
            json!({"max_tokens":512})
        });
        let retry = server
            .mock("POST", "/chat/completions")
            .match_body(request_body.clone())
            .with_status(429)
            .with_header("retry-after", "1ms")
            .with_body("rate limited")
            .create_async()
            .await;
        let (content_type, body) = if streaming {
            let chunk = json!({
                "id": "test", "object": "chat.completion.chunk", "created": 0,
                "model": "token-budget-test",
                "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": "stop"}]
            });
            (
                "text/event-stream",
                format!("data: {chunk}\n\ndata: [DONE]\n\n"),
            )
        } else {
            ("application/json", json!({
                "id": "test", "model": "token-budget-test",
                "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
            }).to_string())
        };
        let accepted = server
            .mock("POST", "/chat/completions")
            .match_body(request_body)
            .with_header("content-type", content_type)
            .with_body(body)
            .create_async()
            .await;
        let mut request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);
        request.max_tokens = Some(4096);
        let response = if streaming {
            provider.complete_stream(&request, &|_| {}).await
        } else {
            provider
                .complete_with_budget(&request, crate::RetryBudget::Background)
                .await
        };
        assert_eq!(response.unwrap().message.text.as_deref(), Some("ok"));

        retry.assert_async().await;
        accepted.assert_async().await;
    }
}
