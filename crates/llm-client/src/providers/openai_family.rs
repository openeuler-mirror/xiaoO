use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use futures::StreamExt;

use crate::convert::{
    llm_request_to_wire, parse_tool_arguments, parsed_chunk_to_stream_chunk,
    wire_response_to_llm_response, wire_usage_to_usage,
};
use crate::error::{
    map_api_status_error, map_reqwest_error_raw as map_reqwest_error, map_serde_error,
    parse_stream_error, ErrorRedactor, LlmError,
};
use crate::providers::stream_utf8::StreamUtf8Decoder;
use crate::url_fallback::{
    build_base_url_candidates, build_final_candidates, build_url_fallback_error_message,
    is_configuration_error, is_http_unauthorized_error, should_try_next_candidate,
    UrlAttemptRecord,
};
use crate::wire_types::{ChatCompletionChunk, ParsedChunk};
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::{
    AssistantMessage, LlmRequest, LlmResponse, ReasoningEffort, StopReason, StreamChunk,
    ToolUseBlock,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAiFamilyAuthStyle {
    Bearer,
}

#[derive(Clone)]
pub(crate) struct OpenAiFamilyProvider {
    client: reqwest::Client,
    api_key: String,
    api_base: String,
    auth_style: OpenAiFamilyAuthStyle,
    default_headers: Vec<(String, String)>,
    reasoning_dialect: ReasoningDialect,
    capabilities: ProviderCapabilities,
    cached_endpoint_url: Arc<RwLock<Option<String>>>,
}

/// Protocol dialect for `reasoning_effort` values.
///
/// OpenAI-compatible endpoints generally accept `xhigh`; Zhipu's
/// chat-completions API only documents `low`/`high`/`max` (`xhigh` is
/// rejected), and its newer models (glm-5.3+) cannot disable thinking — Off
/// can only omit the parameter and take the server default. The dialect is
/// declared per endpoint (not per model name) in the provider registry via
/// [`crate::provider_registry::ProviderProfile::reasoning_dialect`]; adding
/// a Zhipu-dialect endpoint to an OpenAI-compatible provider needs only a
/// registry entry, not a change here. Note: the native Zhipu protocol
/// (`ZhipuProvider`) bypasses the registry and hardcodes its dialect in
/// `zhipu.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningDialect {
    #[default]
    OpenAi,
    Zhipu,
}

impl OpenAiFamilyProvider {
    pub(crate) fn new(
        api_key: String,
        api_base: String,
        model: String,
        auth_style: OpenAiFamilyAuthStyle,
        default_headers: Vec<(String, String)>,
    ) -> Self {
        let skip_tls_verify = crate::http::tls_verification_disabled();
        let max_context_window =
            crate::provider_registry::ProtocolFamily::OpenAiCompatible.static_context_window();
        Self {
            client: reqwest::Client::builder()
                .timeout(crate::request_policy::REQUEST_TIMEOUT)
                .connect_timeout(crate::request_policy::REQUEST_TIMEOUT)
                .danger_accept_invalid_certs(skip_tls_verify)
                .http1_only()
                .build()
                .unwrap_or_else(crate::http::client_build_failed),
            api_key,
            api_base,
            auth_style,
            default_headers,
            reasoning_dialect: ReasoningDialect::default(),
            capabilities: ProviderCapabilities {
                supports_streaming: true,
                supports_tool_calls: true,
                supports_json_mode: true,
                max_context_window,
                model_name: model,
            },
            cached_endpoint_url: Arc::new(RwLock::new(None)),
        }
    }

    /// Overrides the default (OpenAI) dialect; Zhipu-protocol endpoints must
    /// pass [`ReasoningDialect::Zhipu`].
    pub(crate) fn with_reasoning_dialect(mut self, dialect: ReasoningDialect) -> Self {
        self.reasoning_dialect = dialect;
        self
    }

    fn build_body(
        &self,
        request: &LlmRequest,
        force_stream: bool,
    ) -> Result<serde_json::Value, LlmError> {
        let wire = llm_request_to_wire(request, &self.capabilities.model_name);
        let mut body = serde_json::to_value(&wire).map_err(map_serde_error)?;
        let family = classify_model(&self.capabilities.model_name);
        if let Some(reasoning_effort) =
            reasoning_effort_for_model(self.reasoning_dialect, family, request.reasoning_effort)
        {
            body["reasoning_effort"] = serde_json::json!(reasoning_effort);
        }
        if request.reasoning_effort == ReasoningEffort::Off && family == ModelFamily::Glm52 {
            body["thinking"] = serde_json::json!({ "type": "disabled" });
        }
        if force_stream {
            body["stream"] = serde_json::json!(true);
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        Ok(body)
    }

    fn apply_common_headers(&self, mut req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req = req.header("Content-Type", "application/json");
        req = match self.auth_style {
            OpenAiFamilyAuthStyle::Bearer => {
                req.header("Authorization", format!("Bearer {}", self.api_key))
            }
        };
        for (name, value) in &self.default_headers {
            req = req.header(name.as_str(), value);
        }
        req
    }

    async fn try_single_endpoint(
        &self,
        url: &str,
        request: &LlmRequest,
        redactor: &mut ErrorRedactor,
    ) -> Result<LlmResponse, LlmError> {
        redactor.add_url(url);
        let body = self.build_body(request, false)?;
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let request = self
            .apply_common_headers(self.client.post(url))
            .json(&body)
            .build()
            .map_err(map_reqwest_error)?;
        redactor.headers(request.headers());
        let response = self
            .client
            .execute(request)
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        redactor.headers(&headers);
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let resp_body = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(map_api_status_error(
                status,
                &resp_body,
                &body_str,
                Some(&headers),
            ));
        }

        if !content_type.to_lowercase().starts_with("application/json") {
            return Err(LlmError::ApiError(format!(
                "unexpected content type: {content_type}; expected application/json. \
                 Response body preview: {}",
                resp_body.chars().take(200).collect::<String>(),
            )));
        }

        let wire_response: crate::wire_types::WireResponse =
            serde_json::from_str(&resp_body).map_err(map_serde_error)?;
        let mut llm_response = wire_response_to_llm_response(&wire_response);
        llm_response.message.usage = wire_usage_to_usage(&wire_response.usage);
        Ok(llm_response)
    }

    async fn try_stream_endpoint(
        &self,
        url: &str,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        redactor: &mut ErrorRedactor,
    ) -> Result<LlmResponse, LlmError> {
        let started = std::time::Instant::now();
        redactor.add_url(url);
        let body = self.build_body(request, true)?;
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let request = self
            .apply_common_headers(self.client.post(url))
            .json(&body)
            .build()
            .map_err(map_reqwest_error)?;
        redactor.headers(request.headers());
        let response = self
            .client
            .execute(request)
            .await
            .map_err(map_reqwest_error)?;

        let status = response.status();
        let headers = response.headers().clone();
        redactor.headers(&headers);

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(map_api_status_error(
                status,
                &error_body,
                &body_str,
                Some(&headers),
            ));
        }

        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !content_type.to_lowercase().starts_with("text/event-stream") {
            let full_response = response.text().await.unwrap_or_default();
            return Err(crate::error::map_protocol_mismatch(
                url,
                &redactor,
                status.as_u16(),
                &headers,
                format!("unexpected content type: {}", content_type),
                started,
                &full_response,
            ));
        }

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut full_tool_calls = Vec::new();
        let mut final_usage = None;
        let mut final_finish_reason = None;
        let mut final_kv_transfer_params = None;

        let mut decoder = StreamUtf8Decoder::default();
        let mut buffer = String::new();
        let mut byte_stream = response.bytes_stream();
        let mut bytes_received: usize = 0;

        while let Some(chunk_result) = byte_stream.next().await {
            let bytes = chunk_result.map_err(|e| {
                crate::error::map_stream_read_error(
                    url,
                    &redactor,
                    status.as_u16(),
                    &headers,
                    &e,
                    started,
                    bytes_received,
                    &buffer,
                )
            })?;
            bytes_received += bytes.len();
            decoder.push(&bytes, &mut buffer);

            // Cursor-based SSE scanning: find each '\n' without rebuilding
            // the buffer per line; drain the consumed prefix in place.
            let mut start = 0;
            while let Some(relative) = buffer[start..].find('\n') {
                let pos = start + relative;
                let line = &buffer[start..pos];
                start = pos + 1;

                if line.is_empty() {
                    continue;
                }

                if let Some(parsed) = parse_openai_family_stream_line(line)? {
                    if let Some(ref content) = parsed.content {
                        full_text.push_str(content);
                    }
                    if let Some(ref reasoning) = parsed.reasoning {
                        full_reasoning.push_str(reasoning);
                    }
                    if let Some(ref usage) = parsed.usage {
                        final_usage = Some(usage.clone());
                    }
                    if let Some(ref reason) = parsed.finish_reason {
                        final_finish_reason = Some(reason.clone());
                    }
                    if parsed.kv_transfer_params.is_some() {
                        final_kv_transfer_params = parsed.kv_transfer_params.clone();
                    }
                    accumulate_tool_call_deltas(&mut full_tool_calls, &parsed);

                    let stream_chunk = parsed_chunk_to_stream_chunk(&parsed);
                    on_chunk(stream_chunk);
                }
            }
            // Drop the consumed prefix in place; no per-line allocation.
            if start > 0 {
                buffer.drain(..start);
            }
        }

        decoder.finish(&mut buffer);

        let usage = final_usage
            .map(|u| wire_usage_to_usage(&u))
            .unwrap_or_default();

        let stop_reason = match final_finish_reason.as_deref() {
            Some("stop") | Some("end_turn") => StopReason::EndTurn,
            Some("length") | Some("max_tokens") => StopReason::MaxTokens,
            Some("tool_calls") | Some("tool_use") => StopReason::ToolUse,
            Some("content_filter") => StopReason::ContentFilter,
            _ => StopReason::EndTurn,
        };

        let tool_use_blocks: Vec<ToolUseBlock> = full_tool_calls
            .iter()
            .map(|tc| ToolUseBlock {
                call_id: tc.id.clone(),
                tool_name: tc.function.name.clone(),
                input: parse_tool_arguments(&tc.function.arguments),
            })
            .collect();

        if full_text.is_empty() && full_reasoning.is_empty() && tool_use_blocks.is_empty() {
            return Err(LlmError::ApiError(
                "empty stream response: stream completed but received no content. \
                 The endpoint may not support SSE streaming or returned an unexpected response."
                    .to_string(),
            ));
        }

        Ok(LlmResponse {
            message: AssistantMessage {
                text: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                reasoning_content: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning)
                },
                tool_calls: tool_use_blocks,
                usage,
                stop_reason,
            },
            kv_cache_chunk_hashes: final_kv_transfer_params
                .map(|p| p.chunk_hashes)
                .unwrap_or_default(),
        })
    }

    /// Dispatch to the endpoint request matching the call style.
    async fn try_endpoint(
        &self,
        url: &str,
        request: &LlmRequest,
        call: &EndpointCall<'_>,
        redactor: &mut ErrorRedactor,
    ) -> Result<LlmResponse, LlmError> {
        match call {
            EndpointCall::Complete => self.try_single_endpoint(url, request, redactor).await,
            EndpointCall::Stream(on_chunk) => {
                self.try_stream_endpoint(url, request, on_chunk, redactor)
                    .await
            }
        }
    }

    /// Shared endpoint selection flow for `complete` and `complete_stream`:
    /// try the cached endpoint URL once, then walk the URL-fallback candidates
    /// until one succeeds. `call` selects the endpoint request style and
    /// only affects log wording beyond that.
    async fn run_with_url_fallback(
        &self,
        request: &LlmRequest,
        call: EndpointCall<'_>,
    ) -> Result<LlmResponse, LlmError> {
        let mut redactor = ErrorRedactor::new(&self.api_base, &self.api_key);
        let result = async {
            let label = match &call {
                EndpointCall::Complete => "",
                EndpointCall::Stream(_) => " for streaming",
            };

            let cached_url = self
                .cached_endpoint_url
                .read()
                .ok()
                .and_then(|guard| guard.clone());

            if let Some(url) = cached_url {
                // The wrapper owns the retry budget, including cached endpoints.
                return self.try_endpoint(&url, request, &call, &mut redactor).await;
            }

            tracing::info!(
                original_base = %redactor.url(&self.api_base),
                "No cached URL available{} - starting URL fallback to find working endpoint",
                label
            );

            let base_candidates = build_base_url_candidates(&self.api_base);
            let final_urls = build_final_candidates(&base_candidates);

            let mut attempts = Vec::new();

            for (idx, url) in final_urls.iter().enumerate() {
                match self.try_endpoint(url, request, &call, &mut redactor).await {
                    Ok(response) => {
                        tracing::info!(
                            url = %redactor.url(&url),
                            attempt_number = idx + 1,
                            total_candidates = final_urls.len(),
                            "✓ URL fallback succeeded{} - caching this URL for future calls",
                            label
                        );

                        if let Ok(mut guard) = self.cached_endpoint_url.write() {
                            *guard = Some(url.clone());
                        }

                        return Ok(response);
                    }
                    Err(error) => {
                        // Transport failures return to the shared retry policy;
                        // they must not cause additional endpoint probes.
                        match error {
                            LlmError::HttpError(message) => return Err(LlmError::HttpError(
                                format!("{message} | endpoint: {}", redactor.url(url)))),
                            LlmError::Timeout | LlmError::StreamError { .. } | LlmError::IoError(_) => return Err(error),
                            _ => {}
                        }
                        attempts.push(UrlAttemptRecord {
                            index: idx,
                            url: url.clone(),
                            error: error.to_string(),
                        });

                        let is_phase1 = idx < base_candidates.len();
                        let has_more_candidates = idx + 1 < final_urls.len();

                        if has_more_candidates && should_try_next_candidate(&error) {
                            if is_phase1 {
                                tracing::warn!(
                                    "Phase 1 candidate #{} failed: {} - trying next candidate",
                                    idx + 1,
                                    redactor.url(url)
                                );
                            } else {
                                tracing::warn!(
                                    "Phase 2 candidate #{} failed with endpoint error: {} - trying next",
                                    idx + 1,
                                    redactor.url(url)
                                );
                            }
                            continue;
                        }

                        if is_configuration_error(&error) {
                            tracing::error!(
                                error = %redactor.error(error.clone()),
                                url = %redactor.url(&url),
                                "Configuration/parameter error detected{} - not trying more URLs",
                                label
                            );
                            return Err(error);
                        }

                        // Last candidate exhausted with a uniform HTTP 401: surface
                        // the real "Unauthorized" error so the caller gets an
                        // actionable cause instead of the wrapped
                        // "All N candidates failed" summary.
                        if !has_more_candidates && is_http_unauthorized_error(&error) {
                            tracing::error!(
                                url = %redactor.url(&url),
                                error = %redactor.error(error.clone()),
                                "All candidates returned HTTP 401{} - surfacing real auth error instead of wrapped URL fallback",
                                label
                            );
                            return Err(error);
                        }

                        if !should_try_next_candidate(&error) { return Err(error); }
                        let error_msg = build_url_fallback_error_message(&attempts);
                        return Err(LlmError::ApiError(error_msg));
                    }
                }
            }

            // Reachable only when `final_urls` is empty (no candidates generated).
            let error_msg = build_url_fallback_error_message(&attempts);

            Err(LlmError::ApiError(error_msg))
        }
        .await;
        redactor.error_result(result)
    }
}

/// Which endpoint request style a provider call drives.
enum EndpointCall<'a> {
    Complete,
    Stream(&'a (dyn Fn(StreamChunk) + Send + Sync)),
}

/// Model families with known `reasoning_effort` deviations on
/// OpenAI-compatible endpoints, classified by the id's last segment (vendor
/// prefix stripped, case-insensitive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelFamily {
    Gpt56,
    KimiK3,
    Glm52,
    Standard,
}

fn classify_model(model: &str) -> ModelFamily {
    let name = model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with("gpt-5.6") {
        ModelFamily::Gpt56
    } else if name == "kimi-k3" {
        ModelFamily::KimiK3
    } else if name == "glm-5.2" {
        ModelFamily::Glm52
    } else {
        ModelFamily::Standard
    }
}

/// `reasoning_effort` tier → request-body value. High is always `high`; the
/// deviations sit at the Off end (send explicitly or omit) and the Max end
/// (`xhigh` vs `max`):
///
/// | Endpoint / model                | Off     | Max     |
/// |---------------------------------|---------|---------|
/// | generic OpenAI-compatible       | omit    | `xhigh` |
/// | GPT-5.6                         | `none`  | `max`   |
/// | Kimi K3 / GLM-5.2               | omit    | `max`   |
/// | Zhipu dialect (any model)       | omit    | `max`   |
///
/// The table covers only the `reasoning_effort` field; on Off, GLM-5.2's
/// `build_body` additionally sends `thinking: {"type": "disabled"}`
/// (`Glm52` is an exact `classify_model` match — new GLM families on
/// generic endpoints must be registered there).
fn reasoning_effort_for_model(
    dialect: ReasoningDialect,
    family: ModelFamily,
    effort: ReasoningEffort,
) -> Option<&'static str> {
    if dialect == ReasoningDialect::Zhipu {
        // Zhipu officially documents only low/high/max: `xhigh` is rejected;
        // Off omits the parameter and lets the server default take over
        // (glm-5.3+ does not support thinking.type: disabled).
        return match effort {
            ReasoningEffort::Off => None,
            ReasoningEffort::High => Some("high"),
            ReasoningEffort::Max => Some("max"),
        };
    }
    match (effort, family) {
        // GPT-5.6 needs an explicit `none` to turn reasoning off; other
        // models omit the parameter.
        (ReasoningEffort::Off, ModelFamily::Gpt56) => Some("none"),
        (ReasoningEffort::Off, _) => None,
        (ReasoningEffort::High, _) => Some("high"),
        // The newer families only accept `max`; generic endpoints use
        // `xhigh`.
        (ReasoningEffort::Max, ModelFamily::Standard) => Some("xhigh"),
        (ReasoningEffort::Max, _) => Some("max"),
    }
}

#[async_trait]
impl LlmProvider for OpenAiFamilyProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.run_with_url_fallback(request, EndpointCall::Complete)
            .await
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        self.run_with_url_fallback(request, EndpointCall::Stream(on_chunk))
            .await
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

pub(crate) fn parse_openai_family_stream_line(line: &str) -> Result<Option<ParsedChunk>, LlmError> {
    let line = line.trim();

    if line.is_empty() || line.starts_with(':') || line.starts_with("event: ") {
        return Ok(Some(ParsedChunk::default()));
    }

    if line == "[DONE]" {
        return Ok(None);
    }

    let data = match line.strip_prefix("data:") {
        Some(data) => data.trim(),
        None => return Ok(Some(ParsedChunk::default())),
    };

    if data.is_empty() {
        return Ok(Some(ParsedChunk::default()));
    }

    if data == "[DONE]" {
        return Ok(None);
    }

    if let Some(error) = parse_stream_error(data) {
        return Err(error);
    }

    let chunk: ChatCompletionChunk = serde_json::from_str(data)
        .map_err(|e| LlmError::ParseError(format!("Failed to parse stream chunk: {}", e)))?;

    let parsed = if let Some(choice) = chunk.choices.first() {
        ParsedChunk {
            content: choice.delta.content.clone(),
            reasoning: choice.delta.reasoning.clone(),
            finish_reason: choice.finish_reason.clone(),
            usage: chunk.usage.clone(),
            tool_calls: choice.delta.tool_calls.clone(),
            kv_transfer_params: chunk.kv_transfer_params.clone(),
        }
    } else {
        ParsedChunk {
            content: None,
            reasoning: None,
            finish_reason: None,
            usage: chunk.usage.clone(),
            tool_calls: None,
            kv_transfer_params: chunk.kv_transfer_params.clone(),
        }
    };

    Ok(Some(parsed))
}

pub(crate) fn accumulate_tool_call_deltas_pub(
    full_tool_calls: &mut Vec<crate::wire_types::WireToolCall>,
    parsed: &ParsedChunk,
) {
    accumulate_tool_call_deltas(full_tool_calls, parsed);
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/providers/openai_family_test.rs"]
mod tests;

/// Configuration for creating an OpenAI-compatible provider directly.
pub struct OpenAiCompatibleProviderConfig {
    pub api_base: String,
    pub api_key: String,
    pub capabilities: ProviderCapabilities,
}

impl OpenAiCompatibleProviderConfig {
    pub fn new(
        api_base: impl Into<String>,
        api_key: impl Into<String>,
        capabilities: ProviderCapabilities,
    ) -> Self {
        Self {
            api_base: api_base.into(),
            api_key: api_key.into(),
            capabilities,
        }
    }
}

/// A publicly constructable OpenAI-compatible LLM provider.
pub struct OpenAiCompatibleProvider {
    inner: OpenAiFamilyProvider,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: OpenAiCompatibleProviderConfig) -> Result<Self, LlmError> {
        if config.api_base.is_empty() {
            return Err(LlmError::ConfigError(
                "api_base must not be empty".to_string(),
            ));
        }
        if config.api_key.is_empty() {
            return Err(LlmError::ConfigError(
                "api_key must not be empty".to_string(),
            ));
        }
        let model = config.capabilities.model_name.clone();
        let mut inner = OpenAiFamilyProvider::new(
            config.api_key,
            config.api_base,
            model,
            OpenAiFamilyAuthStyle::Bearer,
            Vec::new(),
        );
        inner.capabilities = config.capabilities;
        Ok(Self { inner })
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.inner.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        self.inner.complete_stream(request, on_chunk).await
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        self.inner.capabilities()
    }
}

fn accumulate_tool_call_deltas(
    full_tool_calls: &mut Vec<crate::wire_types::WireToolCall>,
    parsed: &ParsedChunk,
) {
    if let Some(ref deltas) = parsed.tool_calls {
        for delta in deltas {
            let idx = delta.index as usize;
            while full_tool_calls.len() <= idx {
                full_tool_calls.push(crate::wire_types::WireToolCall {
                    id: String::new(),
                    call_type: "function".to_string(),
                    function: crate::wire_types::WireToolCallFunction {
                        name: String::new(),
                        arguments: String::new(),
                    },
                });
            }
            let tc = &mut full_tool_calls[idx];
            if let Some(ref id) = delta.id {
                if !id.is_empty() {
                    tc.id = id.clone();
                }
            }
            if let Some(ref func) = delta.function {
                if let Some(ref name) = func.name {
                    if !name.is_empty() {
                        tc.function.name = name.clone();
                    }
                }
                if let Some(ref args) = func.arguments {
                    tc.function.arguments.push_str(args);
                }
            }
        }
    }
}
