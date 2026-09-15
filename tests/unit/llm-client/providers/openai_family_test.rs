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
fn build_body_omits_reasoning_effort_when_off() {
    let provider = make_provider();
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")])
        .with_reasoning_effort(ReasoningEffort::Off);

    let body = provider.build_body(&request, false).unwrap();

    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn new_model_capability_uses_known_context_window() {
    let provider = OpenAiFamilyProvider::new(
        "test-key".to_string(),
        "https://api.openai.com/v1".to_string(),
        "gpt-5.6-sol".to_string(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );

    assert_eq!(provider.capabilities.max_context_window, 1_050_000);
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
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]).with_tools(vec![
        agent_types::Tool {
            name: "lookup".to_string(),
            description: "Look something up".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        },
    ]);

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
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["reasoning_effort"], "none");
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
    let request = LlmRequest::new(vec![agent_types::ChatMessage::user("hello")]);

    let body = provider.build_body(&request, false).unwrap();

    assert_eq!(body["thinking"]["type"], "disabled");
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
