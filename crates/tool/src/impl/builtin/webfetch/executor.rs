use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use crate::r#impl::reqwest_util::{build_http_client_with_dns_override, format_reqwest_error};

use super::constants::{default_timeout_ms, max_timeout_ms, MAX_RESPONSE_BYTES};
use super::input::{WebFetchFormat, WebFetchInput};
use super::output::WebFetchOutput;
use super::spec::WebFetchToolSpec;
use super::validation;

pub struct WebFetchExecutor {
    spec: Arc<WebFetchToolSpec>,
}

impl WebFetchExecutor {
    pub fn new(spec: Arc<WebFetchToolSpec>) -> Self {
        Self { spec }
    }

    async fn fetch_content(input: &WebFetchInput) -> Result<WebFetchOutput, String> {
        let timeout_ms = input
            .timeout
            .unwrap_or_else(default_timeout_ms)
            .min(max_timeout_ms());

        let mut current_url = url::Url::parse(&input.url)
            .map_err(|error| format!("invalid URL {}: {error}", input.url))?;
        let mut redirects = 0usize;
        let response = loop {
            let url_check = validation::validate_url_string(current_url.as_str());
            if !url_check.result {
                return Err(url_check
                    .message
                    .unwrap_or_else(|| "redirect target is not allowed".to_string()));
            }

            // Resolve once, validate every result, and pin the HTTP client to
            // those exact addresses. This closes the validation/request DNS
            // rebinding gap. A fresh pinned client is built for every redirect.
            let host = current_url
                .host_str()
                .ok_or_else(|| format!("URL has no host: {current_url}"))?;
            let port = current_url
                .port_or_known_default()
                .ok_or_else(|| format!("URL has no usable port: {current_url}"))?;
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
                .await
                .map_err(|error| format!("DNS resolution failed for '{host}': {error}"))?
                .collect();
            if addrs.is_empty() {
                return Err(format!("DNS resolution returned no addresses for '{host}'"));
            }
            let dns_check = validation::validate_resolved_addrs(&addrs);
            if !dns_check.result {
                return Err(dns_check
                    .message
                    .unwrap_or_else(|| "URL resolves to blocked IP".to_string()));
            }

            let client =
                build_http_client_with_dns_override(timeout_ms, Some((host, addrs.as_slice())))?;
            let response = client
                .get(current_url.clone())
                .header(
                    "User-Agent",
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                )
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,\
                     image/avif,image/webp,image/apng,*/*;q=0.8",
                )
                .header("Accept-Language", "en-US,en;q=0.9")
                .send()
                .await
                .map_err(|error| format_reqwest_error(error, &format!("fetching {current_url}")))?;

            if !response.status().is_redirection() {
                break response;
            }
            if redirects >= 10 {
                return Err("too many redirects (maximum 10)".to_string());
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| "redirect response is missing Location header".to_string())?
                .to_str()
                .map_err(|_| "redirect Location header is not valid UTF-8".to_string())?;
            current_url = current_url
                .join(location)
                .map_err(|error| format!("invalid redirect target `{location}`: {error}"))?;
            redirects += 1;
        };

        if !response.status().is_success() {
            return Err(format!(
                "Request failed with status code: {}",
                response.status().as_u16()
            ));
        }

        // Check content-length header before downloading body
        if let Some(content_length) = response.content_length() {
            if content_length as usize > MAX_RESPONSE_BYTES {
                return Err(format!(
                    "Response too large: content-length {}B exceeds {}B limit",
                    content_length, MAX_RESPONSE_BYTES
                ));
            }
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(format!(
                "Response too large: {}B exceeds {}B limit",
                bytes.len(),
                MAX_RESPONSE_BYTES
            ));
        }

        let raw = String::from_utf8_lossy(&bytes).into_owned();
        let is_html = content_type.contains("text/html");

        let content = match &input.format {
            WebFetchFormat::Html => raw,
            WebFetchFormat::Text => {
                if is_html {
                    extract_text_from_html(&raw)
                } else {
                    raw
                }
            }
            WebFetchFormat::Markdown => {
                if is_html {
                    convert_html_to_markdown(&raw)
                } else {
                    format!("```\n{}\n```", raw)
                }
            }
        };

        Ok(WebFetchOutput {
            content,
            url: current_url.to_string(),
            content_type,
            format: input.format.to_string(),
        })
    }
}

fn extract_text_from_html(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120).unwrap_or_default()
}

fn convert_html_to_markdown(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| extract_text_from_html(html))
}

impl Default for WebFetchExecutor {
    fn default() -> Self {
        Self::new(Arc::new(WebFetchToolSpec::new()))
    }
}

#[async_trait]
impl ToolExecutor for WebFetchExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: WebFetchInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let validation_result = validation::validate_input(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);

            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let output = Self::fetch_content(&input)
            .await
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        let serialized =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output: serialized },
        })
    }
}
