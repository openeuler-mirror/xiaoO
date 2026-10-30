use crate::error::{map_reqwest_error, write_stream_error_log, LlmError};
use crate::providers::{
    AnthropicProvider, GeminiProvider, OllamaProvider, OpenAiFamilyAuthStyle, OpenAiFamilyProvider,
    ZhipuProvider,
};
use agent_contracts::LlmProvider;
use agent_llm::{ChatMessageExt, LlmRequestExt};
use agent_types::{ChatMessage, LlmRequest, StopReason};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static HOME_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const SECRETS: &[&str] = &[
    "SENTINEL-KEY",
    "SENTINEL-USERINFO",
    "SENTINEL-BODY",
    "SENTINEL-ECHO-HEADER",
    "SENTINEL-XAPI",
    "SENTINEL-AUTH",
    "SENTINEL-PROXY",
    "SENTINEL-COOKIE",
    "SENTINEL-SETCOOKIE",
    "SENTINEL-TOKEN",
];

struct Home {
    dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
    name: String,
}
impl Home {
    fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", dir.path());
        Self {
            dir,
            previous,
            name: name.into(),
        }
    }
    fn record(&self, result: &str, log_required: bool) -> String {
        let path = self.dir.path().join(".xiaoo/log/error.log");
        let log = std::fs::read_to_string(&path).unwrap_or_default();
        let mode = if path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777
        } else {
            0
        };
        // Export before asserting so a failing regression retains its raw evidence.
        let record = format!(
            "case={}\nhome={}\nmode={mode:o}\nRETURN FULL:\n{result}\nLOG FULL:\n{log}",
            self.name,
            self.dir.path().display()
        );
        if let Some(out) = std::env::var_os("XIAOO_ERROR_TEST_EVIDENCE") {
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(
                std::path::Path::new(&out).join(format!("{}.txt", self.name)),
                record,
            )
            .unwrap();
        }
        let mut files = Vec::new();
        collect_files(self.dir.path(), &mut files);
        let mut leaks = vec![];
        for secret in SECRETS {
            if files.iter().any(|text| text.contains(secret)) || result.contains(secret) {
                leaks.push(*secret);
            }
        }
        assert!(
            leaks.is_empty(),
            "credential leak in {}: {leaks:?}",
            self.name
        );
        assert_eq!(
            path.exists(),
            log_required,
            "unexpected log presence in {}",
            self.name
        );
        if log_required {
            assert_eq!(mode, 0o600);
            assert!(log.contains("source=llm_stream_error"));
            assert!(log.contains(&format!("KEEP-CASE-{}", self.name)));
            assert!(log.contains("HTTP Status: 200"));
            assert!(log.contains("[Response Body]") || log.contains("[Stream Buffer]"));
            assert!(log.contains("KEEP-BODY"));
            let timestamp = log
                .lines()
                .find_map(|line| {
                    line.strip_prefix("===== ")
                        .and_then(|s| s.split_once(" source=").map(|(s, _)| s))
                })
                .unwrap();
            chrono::DateTime::parse_from_rfc3339(timestamp).unwrap();
        }
        log
    }
}
fn collect_files(path: &std::path::Path, output: &mut Vec<String>) {
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_files(&path, output);
        } else {
            output.push(String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned());
        }
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}

struct Endpoint {
    url: String,
    paths: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Endpoint {
    fn new(
        status: u16,
        content_type: &str,
        body: String,
        truncated: bool,
        extra: &str,
        fallback: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let paths = Arc::new(Mutex::new(vec![]));
        let requests = Arc::new(Mutex::new(vec![]));
        let received = requests.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = paths.clone();
        let done = stop.clone();
        let content_type = content_type.to_string();
        let extra = extra.to_string();
        let worker = std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = vec![];
                let mut buf = [0; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let request_text = String::from_utf8_lossy(&request);
                let path = request_text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                let content_length = request_text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let header_end = request
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                received
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request).into_owned());
                seen.lock().unwrap().push(path.clone());
                let code = if fallback && path != "/v1/chat/completions" {
                    404
                } else {
                    status
                };
                let size = body.len() + if truncated { 1000 } else { 0 };
                let response = format!("HTTP/1.1 {code} Test\r\nContent-Type: {content_type}\r\nContent-Length: {size}\r\nConnection: close\r\n{extra}\r\n{body}");
                stream.write_all(response.as_bytes()).ok();
                stream.shutdown(std::net::Shutdown::Both).ok();
            }
        });
        Self {
            url: format!("http://{addr}"),
            paths,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn provider(kind: &str, base: String) -> Box<dyn LlmProvider> {
    let key = "SENTINEL-KEY".to_string();
    let model = "m".to_string();
    match kind {
        "gemini" => Box::new(GeminiProvider::new(key, base, model)),
        "anthropic" => Box::new(AnthropicProvider::new(key, base, model)),
        "ollama" => Box::new(OllamaProvider::new(base, model)),
        "zhipu" => Box::new(ZhipuProvider::new(key, base, model)),
        _ => Box::new(OpenAiFamilyProvider::new(
            key,
            base,
            model,
            OpenAiFamilyAuthStyle::Bearer,
            vec![],
        )),
    }
}
async fn call(p: &dyn LlmProvider, stream: bool) -> Result<agent_types::LlmResponse, LlmError> {
    let request = LlmRequest::new(vec![ChatMessage::user("hello")]);
    if stream {
        p.complete_stream(&request, &|_| {}).await
    } else {
        p.complete(&request).await
    }
}
fn full_error(error: &LlmError) -> String {
    format!("{error}\n{error:?}")
}

async fn interrupted(kind: &str, name: &str, userinfo: bool, echo_headers: bool) {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(name);
    // No terminating newline: the partial event remains in the diagnostic buffer.
    let echo = if kind == "ollama" {
        "SENTINEL-USERINFO"
    } else {
        "SENTINEL-KEY"
    };
    let body = format!(
        "KEEP-CASE-{name} KEEP-BODY echo key={echo} {}",
        if echo_headers {
            "SENTINEL-XAPI SENTINEL-SETCOOKIE"
        } else {
            ""
        }
    );
    let extra = if echo_headers {
        "x-api-key: SENTINEL-XAPI\r\nset-cookie: SENTINEL-SETCOOKIE\r\n"
    } else {
        ""
    };
    let server = Endpoint::new(200, "text/event-stream", body, true, extra, false);
    let base = if userinfo {
        server
            .url
            .replace("http://", "http://user:SENTINEL-USERINFO@")
    } else {
        server.url.clone()
    };
    let error = call(provider(kind, base).as_ref(), true).await.unwrap_err();
    let text = full_error(&error);
    let log = home.record(&text, true);
    let endpoint = reqwest::Url::parse(&server.url).unwrap();
    let logged_url = reqwest::Url::parse(
        log.lines()
            .find_map(|line| line.strip_prefix("URL: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(logged_url.scheme(), endpoint.scheme());
    assert_eq!(logged_url.host_str(), endpoint.host_str());
    assert_eq!(logged_url.port(), endpoint.port());
    assert!(log.contains("Error: error decoding response body"));
    assert!(log.contains("Cause Chain:"));
    assert!(log.contains("Elapsed:"));
    assert!(log.contains("Bytes Received:"));
    assert!(log.contains("Timed Out:"));
    let endpoint_path = match kind {
        "anthropic" => "/messages",
        "gemini" => "/v1beta/models/m:streamGenerateContent",
        "ollama" => "/api/chat",
        _ => "/chat/completions",
    };
    if kind != "openai" && kind != "zhipu" {
        assert_eq!(logged_url.path(), endpoint_path);
    } else {
        // Transport errors no longer trigger endpoint probing.
        assert_eq!(server.paths.lock().unwrap().len(), 1);
    }
    assert_eq!(
        log.matches("source=llm_stream_error").count(),
        server.paths.lock().unwrap().len()
    );
    assert!(matches!(error, LlmError::StreamError { .. }));
}
#[tokio::test]
async fn gemini_interrupted_stream() {
    interrupted("gemini", "gemini_interrupted_stream", false, false).await;
}
#[tokio::test]
async fn anthropic_interrupted_stream() {
    interrupted("anthropic", "anthropic_interrupted_stream", false, false).await;
}
#[tokio::test]
async fn openai_interrupted_stream() {
    interrupted("openai", "openai_interrupted_stream", false, false).await;
}
#[tokio::test]
async fn ollama_interrupted_stream() {
    interrupted("ollama", "ollama_interrupted_stream", true, false).await;
}
#[tokio::test]
async fn anthropic_response_credentials() {
    interrupted("anthropic", "anthropic_response_credentials", false, true).await;
}
#[tokio::test]
async fn zhipu_interrupted_stream() {
    interrupted("zhipu", "zhipu_interrupted_stream", false, false).await;
}

#[tokio::test]
async fn diagnostic_headers_and_body() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("diagnostic_headers_and_body");
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in authentication_headers() {
        headers.insert(name, value.parse().unwrap());
    }
    for (name, value) in [
        ("content-type", "text/event-stream"),
        ("transfer-encoding", "chunked"),
        ("retry-after", "7"),
        ("x-request-id", "KEEP-REQID"),
        ("x-ratelimit-limit", "KEEP-RATELIMIT"),
        ("x-ratelimit-remaining", "KEEP-RATELIMIT"),
        ("x-ratelimit-reset", "KEEP-RATELIMIT"),
        ("openai-model", "KEEP-MODEL"),
    ] {
        headers.insert(name, value.parse().unwrap());
    }
    write_stream_error_log("http://user:SENTINEL-USERINFO@127.0.0.1:9/v1beta/m:streamGenerateContent?alt=sse&key=SENTINEL-KEY&token=SENTINEL-BODY", Some(&headers), "KEEP-CASE-diagnostic_headers_and_body KEEP-BODY SENTINEL-BODY SENTINEL-XAPI SENTINEL-AUTH SENTINEL-PROXY SENTINEL-COOKIE SENTINEL-SETCOOKIE", "stream error SENTINEL-KEY SENTINEL-XAPI SENTINEL-AUTH SENTINEL-PROXY SENTINEL-COOKIE SENTINEL-SETCOOKIE", Some(200));
    let log = home.record("unit logger returned ()", true);
    for value in [
        "x-ratelimit-limit: KEEP-RATELIMIT",
        "x-ratelimit-remaining: KEEP-RATELIMIT",
        "x-ratelimit-reset: KEEP-RATELIMIT",
        "content-type: text/event-stream",
        "transfer-encoding: chunked",
        "KEEP-REQID",
        "KEEP-RATELIMIT",
        "KEEP-MODEL",
        "text/event-stream",
        "chunked",
        "retry-after: 7",
        "/v1beta/m:streamGenerateContent",
        "Length:",
    ] {
        assert!(log.contains(value), "missing {value}");
    }
}

async fn response_error(
    name: &str,
    kind: &str,
    status: u16,
    ct: &str,
    body: &str,
    stream: bool,
    extra: &str,
    log_required: bool,
) -> LlmError {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(name);
    let server = Endpoint::new(
        status,
        ct,
        format!("{body}"),
        false,
        &format!("x-request-id: KEEP-CASE-{name}\r\n{extra}"),
        false,
    );
    let error = call(provider(kind, server.url.clone()).as_ref(), stream)
        .await
        .unwrap_err();
    let log = home.record(&full_error(&error), log_required);
    if log_required {
        assert!(log.contains("unexpected content type"));
        assert!(log.contains("Length:") || log.contains("Unconsumed bytes:"));
    }
    error
}
#[tokio::test]
async fn stream_content_type_error() {
    let error = response_error(
        "stream_content_type_error",
        "openai",
        200,
        "application/json",
        "invalid key SENTINEL-KEY KEEP-BODY",
        true,
        "",
        true,
    )
    .await;
    assert!(matches!(error, LlmError::ApiError(_)));
}
#[tokio::test]
async fn connection_refused_credentials() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("connection_refused_credentials");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let p = provider("gemini", format!("http://{addr}"));
    let mut results = String::new();
    for stream in [false, true] {
        let error = call(p.as_ref(), stream).await.unwrap_err();
        results.push_str(&full_error(&error));
        assert!(matches!(error, LlmError::HttpError(_)));
        assert!(error.to_string().contains("Connection failed"));
        assert!(error.to_string().contains(&addr.to_string()));
        assert!(error.to_string().contains("cause chain:"));
        assert!(error.to_string().contains(" -> "));
        assert!(error.to_string().contains("Connection refused"));
    }
    home.record(&results, false);
}
async fn status_body_credentials(stream: bool) {
    let error = response_error(
        if stream {
            "http_status_stream"
        } else {
            "http_status_complete"
        },
        "gemini",
        400,
        "application/json",
        "invalid API key: SENTINEL-KEY",
        stream,
        "",
        false,
    )
    .await;
    assert!(matches!(error, LlmError::ApiError(_)));
    assert!(error.to_string().contains("HTTP 400"));
}
#[tokio::test]
async fn http_status_stream_credentials() {
    status_body_credentials(true).await;
}
#[tokio::test]
async fn http_status_complete_credentials() {
    status_body_credentials(false).await;
}
async fn rate_body_credentials(kind: &str) {
    let error = response_error(
        &format!("rate_limit_{kind}"),
        kind,
        429,
        "application/json",
        "key SENTINEL-KEY",
        true,
        "retry-after: 7\r\n",
        false,
    )
    .await;
    assert!(matches!(
        error,
        LlmError::RateLimited {
            retry_after_ms: 7000,
            ..
        }
    ));
}
#[tokio::test]
async fn anthropic_rate_body_credentials() {
    rate_body_credentials("anthropic").await;
}
#[tokio::test]
async fn openai_rate_body_credentials() {
    rate_body_credentials("openai").await;
}
#[tokio::test]
async fn complete_content_type_error() {
    let error = response_error(
        "complete_content_type_error",
        "openai",
        200,
        "text/html",
        "invalid SENTINEL-KEY",
        false,
        "",
        false,
    )
    .await;
    assert!(matches!(error, LlmError::ApiError(_)));
    assert!(error.to_string().contains("Response body preview"));
}
#[tokio::test]
async fn fallback_url_credentials() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("fallback_url_credentials");
    let server = Endpoint::new(404, "application/json", "missing".into(), false, "", false);
    let base = format!(
        "{}/v1",
        server
            .url
            .replace("http://", "http://user:SENTINEL-USERINFO@")
    );
    let expected = crate::url_fallback::build_final_candidates(
        &crate::url_fallback::build_base_url_candidates(&base),
    );
    let error = call(provider("openai", base).as_ref(), false)
        .await
        .unwrap_err();
    home.record(&full_error(&error), false);
    assert!(error.to_string().contains("candidates failed"));
    let paths: Vec<_> = expected
        .iter()
        .map(|u| reqwest::Url::parse(u).unwrap().path().to_string())
        .collect();
    assert_eq!(*server.paths.lock().unwrap(), paths);
}
#[tokio::test]
async fn sse_error_credentials() {
    let error = response_error("sse_error_credentials", "openai", 200, "text/event-stream", "data: {\"type\":\"error\",\"error\":{\"code\":\"x\",\"message\":\"key SENTINEL-KEY rejected\"}}\n\n", true, "", false).await;
    assert!(matches!(error, LlmError::ApiError(_)));
    assert!(error.to_string().contains("stream error:"));
}
#[tokio::test]
async fn malformed_json_body() {
    let error = response_error(
        "malformed_json_body",
        "gemini",
        200,
        "application/json",
        "SENTINEL-BODY",
        false,
        "",
        false,
    )
    .await;
    assert!(matches!(error, LlmError::StreamError { .. }));
    assert!(error.to_string().contains("error decoding response body"));
}
#[tokio::test]
async fn short_real_reqwest_timeout() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("short_real_reqwest_timeout");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let error = client
        .get(format!(
            "http://user:SENTINEL-USERINFO@{addr}/?key=SENTINEL-KEY"
        ))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_timeout());
    let error = map_reqwest_error(error);
    home.record(&full_error(&error), false);
    assert!(matches!(error, LlmError::HttpError(_)));
    assert!(error.to_string().contains("Request timeout"));
}
#[tokio::test]
async fn provider_connection_timeout() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("provider_connection_timeout");
    // A local TCP peer accepts but never completes TLS. This exercises the
    // provider's real 20-second timeout without routing assumptions.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let p = provider(
        "openai",
        format!("https://user:SENTINEL-USERINFO@{addr}/chat/completions"),
    );
    let start = Instant::now();
    let error = call(p.as_ref(), true).await.unwrap_err();
    let elapsed = start.elapsed();
    home.record(
        &format!("elapsed_ms={}\n{}", elapsed.as_millis(), full_error(&error)),
        false,
    );
    assert!(matches!(error, LlmError::HttpError(_)));
    assert!(error.to_string().contains("Request timeout"));
    assert!(elapsed >= Duration::from_secs(19) && elapsed < Duration::from_secs(30));
}

fn success_body(kind: &str, stream: bool) -> String {
    let body = match (kind, stream) {
        ("anthropic", false) => r#"{"content":[{"type":"text","text":"KEEP-OK"}],"usage":{"input_tokens":2,"output_tokens":1},"stop_reason":"end_turn"}"#,
        ("gemini", false) => r#"{"candidates":[{"content":{"parts":[{"text":"KEEP-OK"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":1}}"#,
        (_, false) => r#"{"id":"x","object":"chat.completion","created":0,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"KEEP-OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}"#,
        ("anthropic", true) => concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"KEEP-OK\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
        ("gemini", true) => "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"KEEP-OK\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":1}}\n\ndata: [DONE]\n\n",
        (_, true) => "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",\"created\":0,\"model\":\"m\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"KEEP-OK\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}}\n\ndata: [DONE]\n\n",
    };
    body.into()
}
#[tokio::test]
async fn successful_requests_unchanged() {
    let _lock = HOME_LOCK.lock().await;
    for kind in ["anthropic", "gemini", "openai"] {
        for stream in [false, true] {
            let home = Home::new(&format!("success_{kind}_{stream}"));
            let server = Endpoint::new(
                200,
                if stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
                success_body(kind, stream),
                false,
                "",
                false,
            );
            let response = call(provider(kind, server.url.clone()).as_ref(), stream)
                .await
                .unwrap();
            home.record(&format!("{response:?}"), false);
            assert_eq!(response.message.text.as_deref(), Some("KEEP-OK"));
            assert_eq!(response.message.usage.prompt_tokens, 2);
            assert_eq!(response.message.usage.completion_tokens, 1);
            assert_eq!(response.message.usage.total_tokens, 3);
            assert!(matches!(response.message.stop_reason, StopReason::EndTurn));
        }
    }
}
#[tokio::test]
async fn fallback_success_and_cache_unchanged() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("fallback_success_and_cache_unchanged");
    let server = Endpoint::new(
        200,
        "application/json",
        success_body("openai", false),
        false,
        "",
        true,
    );
    let p = provider("openai", server.url.clone());
    let first = call(p.as_ref(), false).await.unwrap();
    let first_paths = server.paths.lock().unwrap().clone();
    let expected = crate::url_fallback::build_final_candidates(
        &crate::url_fallback::build_base_url_candidates(&server.url),
    );
    let expected_paths: Vec<_> = expected
        .iter()
        .map(|u| reqwest::Url::parse(u).unwrap().path().to_string())
        .take_while(|s| s != "/v1/chat/completions")
        .chain(std::iter::once("/v1/chat/completions".into()))
        .collect();
    assert_eq!(first_paths, expected_paths);
    let second = call(p.as_ref(), false).await.unwrap();
    assert_eq!(server.paths.lock().unwrap().len(), first_paths.len() + 1);
    assert_eq!(
        server.paths.lock().unwrap().last().unwrap(),
        "/v1/chat/completions"
    );
    home.record(
        &format!(
            "first={first:?}\nsecond={second:?}\npaths={:?}",
            server.paths.lock().unwrap()
        ),
        false,
    );
    assert_eq!(first.message.text, second.message.text);
}
#[tokio::test]
async fn rate_limit_classification_unchanged() {
    for (status, headers, delay) in [(429, "retry-after: 7\r\n", 7000), (529, "", 5000)] {
        let error = response_error(
            &format!("rate_status_{status}"),
            "openai",
            status,
            "application/json",
            "",
            true,
            headers,
            false,
        )
        .await;
        match error {
            LlmError::RateLimited {
                retry_after_ms,
                message,
            } => {
                assert_eq!(retry_after_ms, delay);
                assert_eq!(message, "no response body");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
#[tokio::test]
async fn context_overflow_classification_unchanged() {
    for (name, status, ct, body) in [
        ("overflow_status", 413, "application/json", ""),
        ("overflow_body", 400, "application/json", "maximum context length is 128000 tokens"),
        ("overflow_sse", 200, "text/event-stream", "data: {\"type\":\"error\",\"error\":{\"code\":\"context_length_exceeded\",\"message\":\"overflow\"}}\n\n"),
    ] {
        let error = response_error(name, "openai", status, ct, body, true, "", false).await;
        assert!(matches!(error, LlmError::ContextLengthExceeded { .. }));
    }
}

#[tokio::test]
async fn encoded_url_credentials() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("encoded_url_credentials");
    write_stream_error_log(
        "http://api:SENTINEL%2dUSERINFO@api.example/path?ApI_Key=SENTINEL%2dKEY&ToKeN=LONGTOKEN&other=LONG&alt=sse",
        None,
        "KEEP-CASE-encoded_url_credentials KEEP-BODY api passed classes sse LONG SENTINEL-USERINFO SENTINEL-KEY LONGTOKEN",
        "encoded SENTINEL-KEY SENTINEL-USERINFO",
        Some(200),
    );
    let log = home.record("unit logger returned ()", true);
    assert!(log.contains("URL: http://api.example/path?ApI_Key=[REDACTED]&ToKeN=[REDACTED]&other=[REDACTED]&alt=[REDACTED]"));
    assert!(log.contains("KEEP-BODY api passed classes sse LONG "));
    for secret in [
        "SENTINEL%2dKEY",
        "SENTINEL-KEY",
        "SENTINEL-USERINFO",
        "LONGTOKEN",
    ] {
        assert!(!log.contains(secret));
    }
}

async fn catalog_credentials(kind: &str) {
    use crate::models::{
        AnthropicModelCatalog, GeminiModelCatalog, ModelCatalog, OllamaModelCatalog,
        OpenAiFamilyModelCatalog,
    };
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(&format!("catalog_{kind}"));
    let body = if kind == "ollama" {
        "SENTINEL-USERINFO SENTINEL-ECHO-HEADER"
    } else {
        "SENTINEL-KEY SENTINEL-USERINFO SENTINEL-ECHO-HEADER"
    };
    let server = Endpoint::new(
        400,
        "application/json",
        body.into(),
        false,
        "x-api-key: SENTINEL-ECHO-HEADER\r\n",
        false,
    );
    let base = server
        .url
        .replace("http://", "http://user:SENTINEL-USERINFO@");
    let key = "SENTINEL-KEY".to_string();
    let catalog: Box<dyn ModelCatalog> = match kind {
        "gemini" => Box::new(GeminiModelCatalog::new(key, base)),
        "anthropic" => Box::new(AnthropicModelCatalog::new(key, base)),
        "ollama" => Box::new(OllamaModelCatalog::new(base)),
        _ => Box::new(OpenAiFamilyModelCatalog::new(key, base)),
    };
    let error = catalog.list_models().await.unwrap_err();
    home.record(&full_error(&error), false);
    assert!(matches!(error, LlmError::ApiError(_)));
    assert!(error.to_string().contains("HTTP 400"));
}
#[tokio::test]
async fn gemini_catalog_credentials() {
    catalog_credentials("gemini").await;
}
#[tokio::test]
async fn anthropic_catalog_credentials() {
    catalog_credentials("anthropic").await;
}
#[tokio::test]
async fn openai_catalog_credentials() {
    catalog_credentials("openai").await;
}
#[tokio::test]
async fn ollama_catalog_credentials() {
    catalog_credentials("ollama").await;
}
#[tokio::test]
async fn ollama_model_details_credentials() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("ollama_model_details_credentials");
    let server = Endpoint::new(
        400,
        "application/json",
        "SENTINEL-USERINFO".into(),
        false,
        "",
        false,
    );
    let catalog = crate::models::OllamaModelCatalog::new(
        server
            .url
            .replace("http://", "http://user:SENTINEL-USERINFO@"),
    );
    let error = catalog.get_context_length("m").await.unwrap_err();
    home.record(&full_error(&error), false);
    assert!(matches!(error, LlmError::ApiError(_)));
}

#[path = "error_boundary_test.rs"]
mod boundaries;

fn authentication_headers() -> [(&'static str, &'static str); 5] {
    [
        ("x-api-key", "SENTINEL-XAPI"),
        ("authorization", "SENTINEL-AUTH"),
        ("proxy-authorization", "SENTINEL-PROXY"),
        ("cookie", "SENTINEL-COOKIE"),
        ("set-cookie", "SENTINEL-SETCOOKIE"),
    ]
}

#[path = "error_content_test.rs"]
mod content;
