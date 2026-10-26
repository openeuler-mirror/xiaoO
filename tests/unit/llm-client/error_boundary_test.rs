use super::*;
use crate::models::{
    AnthropicModelCatalog, GeminiModelCatalog, ModelCatalog, OllamaModelCatalog,
    OpenAiFamilyModelCatalog,
};

#[tokio::test]
async fn malformed_url_credentials() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("malformed_url_credentials");
    let url = "http://user:SENTINEL-USERINFO@localhost:bad/chat/completions?token=SENTINEL-TOKEN&other=KEEP-PARAM";
    let error = call(provider("openai", url.into()).as_ref(), false)
        .await
        .unwrap_err();
    let full = full_error(&error);
    home.record(&full, false);
    assert!(full.contains("localhost:bad/chat/completions"));
    assert!(!full.contains("KEEP-PARAM"));
}

#[tokio::test]
async fn malformed_url_log() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("malformed_url_log");
    write_stream_error_log(
        "http://user:SENTINEL-USERINFO@localhost:bad/path?token=SENTINEL-TOKEN&other=KEEP-PARAM",
        None,
        "KEEP-CASE-malformed_url_log KEEP-BODY SENTINEL-USERINFO SENTINEL-TOKEN KEEP-PARAM",
        "builder error",
        Some(200),
    );
    let log = home.record("unit logger returned ()", true);
    assert!(log.contains("URL: http://localhost:bad/path?token=[REDACTED]&other=[REDACTED]"));
    assert!(log.contains("KEEP-BODY [REDACTED] [REDACTED] KEEP-PARAM"));
}

#[tokio::test]
async fn userinfo_preserves_endpoint_and_text() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("userinfo_preserves_endpoint_and_text");
    write_stream_error_log(
        "http://api:secret@secret.api.example/secret/api?alt=sse",
        None,
        "KEEP-CASE-userinfo_preserves_endpoint_and_text KEEP-BODY api passed classes sse secret",
        "api classes passed",
        Some(200),
    );
    let log = home.record("unit logger returned ()", true);
    assert!(log.contains("URL: http://secret.api.example/secret/api?alt=[REDACTED]"));
    assert!(log.contains("KEEP-BODY api passed classes sse [REDACTED]"));
    assert!(log.contains("Error: api classes passed"));
}

async fn basic_catalog(kind: &str) {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(&format!("basic_catalog_{kind}"));
    // Derive the value from the same public request builder used by the catalog.
    let probe_url = "http://user:SENTINEL-USERINFO@127.0.0.1/";
    let request = reqwest::Client::new().get(probe_url).build().unwrap();
    let basic = request
        .headers()
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    let server = Endpoint::new(
        400,
        "application/json",
        format!("echo {basic} KEEP-BODY"),
        false,
        "",
        false,
    );
    let base = server
        .url
        .replace("http://", "http://user:SENTINEL-USERINFO@");
    let catalog: Box<dyn ModelCatalog> = match kind {
        "anthropic" => Box::new(AnthropicModelCatalog::new("SENTINEL-KEY".into(), base)),
        "gemini" => Box::new(GeminiModelCatalog::new("SENTINEL-KEY".into(), base)),
        "ollama" => Box::new(OllamaModelCatalog::new(base)),
        // An empty optional key leaves the actual Basic header in use.
        _ => Box::new(OpenAiFamilyModelCatalog::new(String::new(), base)),
    };
    let error = catalog.list_models().await.unwrap_err();
    assert!(received_header(&server, "authorization", basic));
    let full = full_error(&error);
    home.record(&full, false);
    assert!(!full.contains(basic), "complete Basic header leaked");
    assert!(full.contains("HTTP 400"));
    assert!(full.contains("KEEP-BODY"));
}
#[tokio::test]
async fn anthropic_basic_catalog() {
    basic_catalog("anthropic").await;
}
#[tokio::test]
async fn gemini_basic_catalog() {
    basic_catalog("gemini").await;
}
#[tokio::test]
async fn ollama_basic_catalog() {
    basic_catalog("ollama").await;
}
#[tokio::test]
async fn openai_basic_catalog() {
    basic_catalog("openai").await;
}
#[tokio::test]
async fn ollama_basic_model_details() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("ollama_basic_model_details");
    let request = reqwest::Client::new()
        .get("http://user:SENTINEL-USERINFO@127.0.0.1/")
        .build()
        .unwrap();
    let basic = request
        .headers()
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    let server = Endpoint::new(400, "application/json", basic.into(), false, "", false);
    let catalog = OllamaModelCatalog::new(
        server
            .url
            .replace("http://", "http://user:SENTINEL-USERINFO@"),
    );
    let error = catalog.get_context_length("m").await.unwrap_err();
    assert!(received_header(&server, "authorization", basic));
    let full = full_error(&error);
    home.record(&full, false);
    assert!(!full.contains(basic));
}

async fn encoded_response(name: &str, stream: bool, encoded: &str) {
    let body = if stream {
        format!("data: {{\"type\":\"error\",\"error\":{{\"code\":\"x\",\"message\":\"key {encoded} rejected\"}}}}\n\n")
    } else {
        format!("{{\"error\":\"key {encoded} rejected\"}}")
    };
    let error = response_error(
        name,
        "openai",
        if stream { 200 } else { 400 },
        if stream {
            "text/event-stream"
        } else {
            "application/json"
        },
        &body,
        stream,
        "",
        false,
    )
    .await;
    assert!(matches!(error, LlmError::ApiError(_)));
    let full = full_error(&error);
    assert!(!full.contains(encoded), "encoded credential leaked");
    assert!(!full.contains("SENTINEL-KEY"));
}
#[tokio::test]
async fn percent_status_error() {
    encoded_response("percent_status_error", false, "SENTINEL%2dKEY").await;
}
#[tokio::test]
async fn unicode_status_error() {
    encoded_response("unicode_status_error", false, r"SENTINEL\u002dKEY").await;
}
#[tokio::test]
async fn percent_sse_error() {
    encoded_response("percent_sse_error", true, "SENTINEL%2DKEY").await;
}
#[tokio::test]
async fn unicode_sse_error() {
    encoded_response("unicode_sse_error", true, r"SENTINEL\u002dKEY").await;
}

#[tokio::test]
async fn unicode_surrogates_and_mixed_encodings() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("unicode_surrogates_and_mixed_encodings");
    let body = r"KEEP-CASE-unicode_surrogates_and_mixed_encodings KEEP-BODY \u5bc6\u94A5\ud83d\ude00 %e5%af%86%E9%92%A5%f0%9f%98%80 \u5bc6%E9%92%a5😀";
    write_stream_error_log(
        "http://127.0.0.1:9/path?token=%E5%AF%86%E9%92%A5%F0%9F%98%80",
        None,
        body,
        "stream error",
        Some(200),
    );
    let log = home.record("unit logger returned ()", true);
    assert!(log.contains("KEEP-BODY [REDACTED] [REDACTED] [REDACTED]"));
    assert!(!log.contains("密钥😀"));
}

#[derive(Clone)]
pub(super) struct TraceCapture(pub(super) Arc<Mutex<String>>);
struct Fields<'a>(&'a mut String);
impl tracing::field::Visit for Fields<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        write!(self.0, " {}={value:?}", field.name()).unwrap();
    }
}
impl tracing::Subscriber for TraceCapture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
        Some(tracing::metadata::LevelFilter::TRACE)
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut text = self.0.lock().unwrap();
        // Only llm-client events belong to this output contract.
        if event.metadata().target().starts_with("llm_client") {
            event.record(&mut Fields(&mut text));
            text.push('\n');
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn tracing_fallback_urls() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("tracing_fallback_urls");
    let capture = TraceCapture(Arc::new(Mutex::new(String::new())));
    let _guard = tracing::subscriber::set_default(capture.clone());
    let server = Endpoint::new(
        404,
        "application/json",
        "KEEP-BODY".into(),
        false,
        "",
        false,
    );
    let base = format!(
        "{}/v1?token=SENTINEL-TOKEN",
        server
            .url
            .replace("http://", "http://user:SENTINEL-USERINFO@")
    );
    let error = call(provider("openai", base).as_ref(), false)
        .await
        .unwrap_err();
    let trace = capture.0.lock().unwrap().clone();
    home.record(&format!("{}\nTRACING:\n{trace}", full_error(&error)), false);
    assert!(trace.contains("original_base"));
    assert!(trace.contains("candidate"));
    assert!(trace.contains(&server.url));
}

async fn fallback_overlap(key: &str, status: u16, body: &str, name: &str) {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(name);
    let server = Endpoint::new(status, "application/json", body.into(), false, "", false);
    let p = OpenAiFamilyProvider::new(
        key.into(),
        server.url.clone(),
        "m".into(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    let error = call(&p, false).await.unwrap_err();
    let paths = server.paths.lock().unwrap().clone();
    let expected = crate::url_fallback::build_final_candidates(
        &crate::url_fallback::build_base_url_candidates(&server.url),
    );
    home.record(
        &format!(
            "{}\nrequests={}\npaths={paths:?}\nexpected_count={}",
            full_error(&error),
            paths.len(),
            expected.len()
        ),
        false,
    );
    assert_eq!(paths.len(), expected.len());
    if status == 401 {
        assert!(!error.to_string().contains("candidates failed"));
    }
}
#[tokio::test]
async fn fallback_status_overlaps_credential() {
    fallback_overlap(
        "401",
        401,
        "KEEP-BODY",
        "fallback_status_overlaps_credential",
    )
    .await;
}
#[tokio::test]
async fn fallback_keyword_overlaps_credential() {
    fallback_overlap(
        "url error",
        400,
        r#"{"error":{"message":"url error"}}"#,
        "fallback_keyword_overlaps_credential",
    )
    .await;
}

async fn cached_retry(key: &str, status: usize, body: &str, name: &str, trace_check: bool) {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(name);
    let capture = TraceCapture(Arc::new(Mutex::new(String::new())));
    let _guard = tracing::subscriber::set_default(capture.clone());
    let mut server = mockito::Server::new_async().await;
    let mut good = server
        .mock("POST", mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(success_body("openai", false))
        .expect(1)
        .create_async()
        .await;
    let base = if trace_check {
        format!(
            "{}/chat/completions?token=SENTINEL-TOKEN",
            server
                .url()
                .replace("http://", "http://user:SENTINEL-USERINFO@")
        )
    } else {
        format!("{}/chat/completions", server.url())
    };
    let p = OpenAiFamilyProvider::new(
        key.into(),
        base,
        "m".into(),
        OpenAiFamilyAuthStyle::Bearer,
        vec![],
    );
    call(&p, false).await.unwrap();
    good.assert_async().await;
    good.remove_async().await;
    let failure = server
        .mock("POST", mockito::Matcher::Any)
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(4)
        .create_async()
        .await;
    let error = call(&p, false).await.unwrap_err();
    let trace = capture.0.lock().unwrap().clone();
    home.record(&format!("{}\nTRACING:\n{trace}", full_error(&error)), false);
    failure.assert_async().await;
    if trace_check {
        assert!(trace.contains("Retrying cached URL"));
        assert!(trace.contains("All 3 retry attempts failed"));
    }
}
#[tokio::test]
async fn retry_status_overlaps_credential() {
    cached_retry(
        "502 Bad Gateway",
        502,
        "KEEP-BODY",
        "retry_status_overlaps_credential",
        false,
    )
    .await;
}
#[tokio::test]
async fn retry_keyword_overlaps_credential() {
    cached_retry(
        "network",
        500,
        "network",
        "retry_keyword_overlaps_credential",
        false,
    )
    .await;
}
#[tokio::test]
async fn tracing_cached_retry_urls() {
    cached_retry(
        "SENTINEL-KEY",
        503,
        "KEEP-BODY",
        "tracing_cached_retry_urls",
        true,
    )
    .await;
}

#[tokio::test]
async fn response_authentication_values_independent() {
    let headers = authentication_headers()
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let body = authentication_headers()
        .iter()
        .map(|(_, value)| *value)
        .collect::<Vec<_>>()
        .join(" ");
    response_error(
        "response_authentication_values_independent",
        "anthropic",
        400,
        "application/json",
        &body,
        false,
        &headers,
        false,
    )
    .await;
}
#[tokio::test]
async fn request_authentication_values_independent() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("request_authentication_values_independent");
    let body = authentication_headers()
        .iter()
        .map(|(_, value)| *value)
        .collect::<Vec<_>>()
        .join(" ");
    let server = Endpoint::new(400, "application/json", body, false, "", false);
    let headers = authentication_headers()
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    let p = OpenAiFamilyProvider::new(
        "SENTINEL-KEY".into(),
        server.url.clone(),
        "m".into(),
        OpenAiFamilyAuthStyle::Bearer,
        headers,
    );
    let error = call(&p, false).await.unwrap_err();
    for (name, value) in authentication_headers() {
        assert!(received_header(&server, name, value));
    }
    home.record(&full_error(&error), false);
}

#[tokio::test]
async fn log_descriptor_is_private_before_writes() {
    use std::os::unix::fs::PermissionsExt;
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("log_descriptor_is_private_before_writes");
    let path = home.dir.path().join("created.log");
    let file = crate::error::open_error_log(&path).unwrap();
    let mode = file.metadata().unwrap().permissions().mode() & 0o777;
    let len = file.metadata().unwrap().len();
    home.record(
        &format!("mode_before_write={mode:o}\nlength_before_write={len}"),
        false,
    );
    assert_eq!(len, 0);
    assert_eq!(mode, 0o600);
}

fn received_header(server: &Endpoint, name: &str, value: &str) -> bool {
    server.requests.lock().unwrap().iter().any(|request| {
        request.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(n, v)| n.eq_ignore_ascii_case(name) && v.trim() == value)
        })
    })
}
