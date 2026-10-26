use super::*;
use crate::error::ErrorRedactor;

fn record_output(name: &str, output: &str) {
    if let Some(out) = std::env::var_os("XIAOO_ERROR_TEST_EVIDENCE") {
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(
            std::path::Path::new(&out).join(format!("{name}.txt")),
            output,
        )
        .unwrap();
    }
}

#[tokio::test]
async fn unrelated_links_redact_credentials_in_all_components() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("unrelated_links_redact_credentials_in_all_components");
    let capture = super::boundaries::TraceCapture(Arc::new(Mutex::new(String::new())));
    let _guard = tracing::subscriber::set_default(capture.clone());
    let body = "https://docs.example/echo/SENTINEL-KEY https://docs.example/help?value=SENTINEL-KEY https://docs.example/help#SENTINEL-KEY";
    let error = response_error_unlocked(&home, "openai", body).await;
    let trace = capture.0.lock().unwrap().clone();
    let full = format!("{}\nTRACING:\n{trace}", full_error(&error));
    home.record(&full, false);
    assert!(trace.contains("Configuration/parameter error"));
    for expected in [
        "https://docs.example/echo/[REDACTED]",
        "https://docs.example/help?value=[REDACTED]",
        "https://docs.example/help#[REDACTED]",
    ] {
        assert!(full.contains(expected));
    }
}

async fn response_error_unlocked(home: &Home, kind: &str, body: &str) -> LlmError {
    let _ = home;
    let server = Endpoint::new(400, "application/json", body.into(), false, "", false);
    call(provider(kind, server.url.clone()).as_ref(), false)
        .await
        .unwrap_err()
}

#[tokio::test]
async fn unrelated_link_in_dedicated_log() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("unrelated_link_in_dedicated_log");
    let redactor = ErrorRedactor::new("http://127.0.0.1:9/chat/completions", "SENTINEL-KEY");
    let text = "KEEP-CASE-unrelated_link_in_dedicated_log KEEP-BODY https://docs.example/SENTINEL-KEY?lang=zh#limits";
    crate::error::write_redacted_stream_error_log(
        "http://127.0.0.1:9/chat/completions",
        None,
        text,
        text,
        Some(200),
        &redactor,
    );
    let output = redactor.error(LlmError::ApiError(text.into()));
    let log = home.record(&full_error(&output), true);
    assert!(log.contains("https://docs.example/[REDACTED]?lang=zh#limits"));
}

#[tokio::test]
async fn unrelated_links_remain_exact() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("unrelated_links_remain_exact");
    let link = "https://docs.example/help?lang=zh&section=quota#limits";
    let error = response_error_unlocked(&home, "anthropic", link).await;
    home.record(&full_error(&error), false);
    assert!(error.to_string().contains(link));
}

#[tokio::test]
async fn request_url_prefix_does_not_protect_another_url() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("request_url_prefix_does_not_protect_another_url");
    let redactor = ErrorRedactor::new("https://api.example/v1", "SENTINEL-KEY");
    let body = "https://api.example/v1/docs/SENTINEL-KEY?lang=zh#limits";
    let full = full_error(&redactor.error(LlmError::ApiError(body.into())));
    let request = "https://api.example/v1?alt=sse&key=SENTINEL-KEY";
    let nested = format!("https://docs.example/link?next={request}");
    let redactor = ErrorRedactor::new(request, "SENTINEL-KEY");
    let nested = full_error(&redactor.error(LlmError::ApiError(nested)));
    home.record(&format!("{full}\nNESTED:\n{nested}"), false);
    assert!(full.contains("https://api.example/v1/docs/[REDACTED]?lang=zh#limits"));
    assert!(nested
        .contains("https://docs.example/link?next=https://api.example/v1?alt=sse&key=[REDACTED]"));
}

#[tokio::test]
async fn unrecognized_nested_request_url_never_panics() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("unrecognized_nested_request_url_never_panics");
    let inputs = [
        "not-a-url?next=http://user:secret@host/path",
        "?x=http://a:b@c/a?z=http://d:e@f/",
        "prefix https://user:secret@host/path?x=y",
        "a?b=c://user:secret@host/path",
        "http://user:secret@host:bad/?next=https://a:b@c/?token=secret",
        "http://user:secret@host/path?next=http://a:b@c/path?query=x#fragment",
        "汉字?next=http://user:secret@host/path",
        "http:///missing?key=secret",
        "://",
    ];
    let mut records = String::new();
    let mut panics = vec![];
    for (i, url) in inputs.iter().enumerate() {
        let result = std::panic::catch_unwind(|| {
            let redactor = ErrorRedactor::new(url, "secret");
            (
                redactor.url(url),
                redactor
                    .error(LlmError::HttpError(format!("request failed: {url}")))
                    .to_string(),
            )
        });
        match result {
            Ok((url, error)) => {
                records.push_str(&format!("case={i} URL={url}\nerror={error}\n"));
                assert!(!error.contains("secret"));
            }
            Err(_) => {
                panics.push(i);
                records.push_str(&format!("case={i} PANIC\n"));
            }
        }
    }
    home.record(&records, false);
    assert!(panics.is_empty(), "panic cases: {panics:?}");
    assert!(records.contains("case=0 URL=[invalid URL]"));
}

#[tokio::test]
async fn arbitrary_url_and_text_inputs_never_panic() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("arbitrary_url_and_text_inputs_never_panic");
    let atoms = [
        "",
        "a",
        "?",
        "#",
        "://",
        "@",
        "http://",
        "user:secret@",
        "/",
        "密钥",
        "\\u002f",
        "%FF",
        "\\",
        "=",
        "&",
        "[",
        "]",
    ];
    let mut checked = 0;
    let mut panics = 0;
    for a in atoms {
        for b in atoms {
            for c in atoms {
                let input = format!("{a}{b}{c}");
                if std::panic::catch_unwind(|| {
                    let redactor = ErrorRedactor::new(&input, "secret");
                    redactor.url(&input);
                    redactor.diagnostic(&format!("error={input} https://other.example/{input}"));
                })
                .is_err()
                {
                    panics += 1;
                }
                checked += 1;
            }
        }
    }
    home.record(&format!("inputs={checked}\npanics={panics}"), false);
    assert_eq!(panics, 0);
}

async fn binary_authentication_header(value: &[u8], name: &str, body: String) {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new(name);
    let mut headers = reqwest::header::HeaderMap::new();
    for header in [
        "x-api-key",
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
    ] {
        headers.clear();
        headers.insert(
            header,
            reqwest::header::HeaderValue::from_bytes(value).unwrap(),
        );
        assert!(headers.get(header).unwrap().to_str().is_err());
        let mut redactor = ErrorRedactor::new("http://127.0.0.1:9/", "CONFIG-KEY-DIFFERENT");
        redactor.headers(&headers);
        let full = full_error(&redactor.error(LlmError::ApiError(body.clone())));
        record_output(&format!("{name}-{header}"), &full);
        assert!(!full.contains(&String::from_utf8_lossy(value).to_string()));
        assert!(full.contains("KEEP-BODY"));
    }
    home.record("five distinct header names checked independently", false);
}
#[tokio::test]
async fn utf8_authentication_headers() {
    binary_authentication_header(
        "密钥-SECOND".as_bytes(),
        "utf8_authentication_headers",
        "KEEP-BODY 密钥-SECOND".into(),
    )
    .await;
}
#[tokio::test]
async fn lossy_authentication_headers() {
    binary_authentication_header(
        b"\xff-SECOND",
        "lossy_authentication_headers",
        "KEEP-BODY �-SECOND".into(),
    )
    .await;
}
#[tokio::test]
async fn percent_encoded_non_utf8_authentication_header() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("percent_encoded_non_utf8_authentication_header");
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-api-key",
        reqwest::header::HeaderValue::from_bytes(b"\xff-SECOND").unwrap(),
    );
    let mut redactor = ErrorRedactor::new("http://127.0.0.1:9/", "CONFIG-KEY-DIFFERENT");
    redactor.headers(&headers);
    let full = full_error(&redactor.error(LlmError::ApiError("KEEP-BODY %ff-SECOND".into())));
    home.record(&full, false);
    assert!(!full.contains("%ff-SECOND"));
}

#[tokio::test]
async fn utf8_response_header_through_provider() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("utf8_response_header_through_provider");
    let server = Endpoint::new(
        400,
        "application/json",
        "KEEP-BODY 密钥-SECOND".into(),
        false,
        "x-api-key: 密钥-SECOND\r\n",
        false,
    );
    let error = call(provider("anthropic", server.url.clone()).as_ref(), false)
        .await
        .unwrap_err();
    let full = full_error(&error);
    home.record(&full, false);
    assert!(!full.contains("密钥-SECOND"));
    assert!(full.contains("HTTP 400"));
}

#[tokio::test]
async fn created_log_is_private_without_chmod() {
    use std::os::unix::fs::PermissionsExt;
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("created_log_is_private_without_chmod");
    // This entry point only opens the file. Existing-file hardening occurs elsewhere.
    let path = home.dir.path().join("created.log");
    let file = crate::error::create_error_log(&path).unwrap();
    let mode = file.metadata().unwrap().permissions().mode() & 0o777;
    home.record(
        &format!(
            "creation_mode={mode:o}\nlength={}",
            file.metadata().unwrap().len()
        ),
        false,
    );
    assert_eq!(mode, 0o600);
    assert_eq!(file.metadata().unwrap().len(), 0);
}

fn memory_kib(field: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix(field)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|v| v.parse().ok())
        })
        .unwrap()
}

async fn large_body(escaped: bool) {
    let _lock = HOME_LOCK.lock().await;
    let name = if escaped {
        "large_escaped_body"
    } else {
        "large_plain_body"
    };
    let home = Home::new(name);
    let mib: usize = std::env::var("XIAOO_ERROR_BODY_MIB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let before = memory_kib("VmRSS:");
    let unit = if escaped { r"\u0078" } else { "x" };
    let size = mib * 1024 * 1024;
    let mut body = String::with_capacity(size + 256);
    body.push_str(&format!("KEEP-CASE-{name} KEEP-BODY "));
    body.push_str(&unit.repeat((size - body.len() - 64) / unit.len()));
    body.push_str(" SENTINEL-KEY END-BODY");
    let expected_x = body.bytes().filter(|&byte| byte == b'x').count();
    let redactor = ErrorRedactor::new("http://127.0.0.1:9/chat/completions", "SENTINEL-KEY");
    let start = Instant::now();
    crate::error::write_redacted_stream_error_log(
        "http://127.0.0.1:9/chat/completions",
        None,
        &body,
        "stream failed",
        Some(200),
        &redactor,
    );
    let elapsed = start.elapsed();
    let peak = memory_kib("VmHWM:");
    let auxiliary_peak = (peak.saturating_sub(before) * 1024).saturating_sub(body.len() as u64);
    let path = home.dir.path().join(".xiaoo/log/error.log");
    let length = std::fs::metadata(&path).unwrap().len();
    record_output(name, &format!("body_bytes={}\nlog_bytes={length}\nrss_before_kib={before}\npeak_rss_kib={peak}\nauxiliary_peak_bytes={auxiliary_peak}\nelapsed_ms={}\nisolated={}\n", body.len(), elapsed.as_millis(), std::env::var_os("XIAOO_MEASURE_ERROR_MEMORY").is_some()));
    if let Some(out) = std::env::var_os("XIAOO_ERROR_TEST_EVIDENCE") {
        std::fs::copy(
            &path,
            std::path::Path::new(&out).join(format!("{name}-full.log")),
        )
        .unwrap();
    }
    // Verify full output in bounded chunks; do not allocate another body-sized view.
    let mut file = std::fs::File::open(&path).unwrap();
    let mut buffer = [0; 65536];
    let mut tail = Vec::new();
    let mut x_count = 0;
    let mut end_seen = false;
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        x_count += buffer[..n].iter().filter(|&&byte| byte == b'x').count();
        tail.extend_from_slice(&buffer[..n]);
        assert!(!tail
            .windows(b"SENTINEL-KEY".len())
            .any(|s| s == b"SENTINEL-KEY"));
        end_seen |= tail.windows(b"END-BODY".len()).any(|s| s == b"END-BODY");
        if tail.len() > 32 {
            tail.drain(..tail.len() - 32);
        }
    }
    assert_eq!(x_count, expected_x);
    assert!(end_seen);
    assert!(length >= body.len() as u64);
    if std::env::var_os("XIAOO_MEASURE_ERROR_MEMORY").is_some() {
        assert!(
            auxiliary_peak <= 6 * body.len() as u64,
            "auxiliary memory exceeds six body sizes: {auxiliary_peak}"
        );
    }
}
#[tokio::test]
async fn large_plain_body() {
    large_body(false).await;
}
#[tokio::test]
async fn large_escaped_body() {
    large_body(true).await;
}

#[tokio::test]
async fn literal_escape_sequences_in_credentials() {
    let _lock = HOME_LOCK.lock().await;
    let home = Home::new("literal_escape_sequences_in_credentials");
    let mut output = String::new();
    for (key, encoded) in [
        ("KEY%2fTOKEN", "KEY%252fTOKEN"),
        (r"KEY\u002fTOKEN", r"KEY\\u002fTOKEN"),
    ] {
        let redactor = ErrorRedactor::new("http://127.0.0.1/", key);
        let text = format!("{key} {encoded} KEEP-BODY KEY/TOKEN");
        let error = redactor.error(LlmError::ApiError(text));
        output.push_str(&full_error(&error));
    }
    home.record(&output, false);
    assert_eq!(output.matches("[REDACTED]").count(), 8);
    assert_eq!(output.matches("KEEP-BODY KEY/TOKEN").count(), 4);
}

pub(super) fn percent_encode(bytes: &[u8], upper: bool) -> String {
    bytes
        .iter()
        .map(|&byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                (byte as char).to_string()
            } else if upper {
                format!("%{byte:02X}")
            } else {
                format!("%{byte:02x}")
            }
        })
        .collect()
}

#[test]
fn standard_credential_spellings() {
    let mut records = String::new();
    for key in [
        "KEY%2fTOKEN",
        r"KEY\u002fTOKEN",
        "KEY+/\"\\\n\t\u{0001}密钥",
    ] {
        let redactor = ErrorRedactor::new("http://127.0.0.1/", key);
        let json = serde_json::to_string(key).unwrap();
        for spelling in [
            key.to_string(),
            json[1..json.len() - 1].to_string(),
            percent_encode(key.as_bytes(), true),
            percent_encode(key.as_bytes(), false),
        ] {
            let output = redactor
                .text(&format!("KEEP-BODY {spelling} END"))
                .into_owned();
            records.push_str(&format!("{output}\n"));
            assert_eq!(output, "KEEP-BODY [REDACTED] END");
        }
    }
    record_output("standard_credential_spellings", &records);
}

#[test]
fn nonstandard_echo_encodings_remain_literal() {
    let redactor = ErrorRedactor::new("http://127.0.0.1/", "SENTINEL-KEY");
    let text = r#"{"message":"\\u0053ENTINEL-KEY","partial":"SENTINEL%2dKEY","unicode":"\u0053ENTINEL-KEY"}"#;
    let output = redactor.diagnostic(text);
    record_output("nonstandard_echo_encodings_remain_literal", &output);
    assert_eq!(output, text);
    assert!(serde_json::from_str::<serde_json::Value>(&output).is_ok());
}

#[test]
fn json_quoted_request_url_keeps_host_and_path() {
    let url = "http://secret.example/secret?alt=sse&key=secret";
    let redactor = ErrorRedactor::new(url, "secret");
    let text = serde_json::to_string(&format!("endpoint \"{url}\" denied secret")).unwrap();
    let output = redactor.diagnostic(&text);
    record_output("json_quoted_request_url_keeps_host_and_path", &output);
    assert_eq!(
        output,
        r#""endpoint \"http://secret.example/secret?alt=[REDACTED]&key=[REDACTED]\" denied [REDACTED]""#
    );
    assert!(serde_json::from_str::<String>(&output).is_ok());
}

#[test]
fn standard_json_request_url_spelling() {
    let url = "http://secret.example/secret?alt=\"sse\"&key=secret";
    let redactor = ErrorRedactor::new(url, "secret");
    let text = serde_json::to_string(&format!("endpoint \"{url}\" denied secret")).unwrap();
    let output = redactor.diagnostic(&text);
    record_output("standard_json_request_url_spelling", &output);
    assert_eq!(
        output,
        r#""endpoint \"http://secret.example/secret?alt=[REDACTED]&key=[REDACTED]\" denied [REDACTED]""#
    );
    assert!(serde_json::from_str::<String>(&output).is_ok());
}

#[test]
fn dense_credential_prefix_search() {
    // A dense first byte plus an unrelated percent sign must remain a linear scan.
    let key = "SENTINEL-KEY+/\"\\";
    let redactor = ErrorRedactor::new("http://127.0.0.1/", key);
    redactor.diagnostic("warm up S%\\");
    let sizes: Vec<usize> = std::env::var("XIAOO_DENSE_BYTES")
        .ok()
        .map(|value| vec![value.parse().unwrap()])
        .unwrap_or_else(|| vec![65536]);
    let rounds: usize = std::env::var("XIAOO_DENSE_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let mut records = String::new();
    for size in sizes {
        let body = format!("{}%", "S".repeat(size));
        let before = memory_kib("VmRSS:");
        let mut times = vec![];
        for _ in 0..rounds {
            let start = Instant::now();
            let output = redactor.diagnostic(&body);
            times.push(start.elapsed().as_nanos());
            assert_eq!(output, body);
            assert!(matches!(output, std::borrow::Cow::Borrowed(_)));
        }
        records.push_str(&format!("bytes={} rounds={rounds} elapsed_ns={times:?} rss_before_kib={before} peak_rss_kib={} unchanged=true\n", body.len(), memory_kib("VmHWM:")));
    }
    record_output("dense_credential_prefix_search", &records);
}

#[test]
fn json_control_spellings_respect_escape_boundaries() {
    let mut records = String::new();
    let mut failures = vec![];
    for key in [
        "\tSECOND",
        "\nSECOND",
        "\rSECOND",
        "\u{0008}SECOND",
        "\u{000c}SECOND",
        "\u{0001}SECOND",
    ] {
        let redactor = ErrorRedactor::new("http://api.example/", key);
        let encoded = serde_json::to_string(key).unwrap();
        let spelling = &encoded[1..encoded.len() - 1];
        for preceding in 0..9 {
            let prefix = "\\".repeat(preceding);
            let input = format!(r#"{{"message":"{prefix}{spelling}"}}"#);
            let expected = if preceding % 2 == 0 {
                format!(r#"{{"message":"{prefix}[REDACTED]"}}"#)
            } else {
                input.clone()
            };
            let output = redactor.diagnostic(&input);
            let valid = serde_json::from_str::<serde_json::Value>(&output).is_ok();
            records.push_str(&format!("spelling={spelling} preceding={preceding} valid_json={valid}\ninput={input}\noutput={output}\nexpected={expected}\n"));
            if output != expected || !valid {
                failures.push((spelling.to_string(), preceding));
            }
        }
        let raw = format!("KEEP {key} END");
        let output = redactor.text(&raw);
        records.push_str(&format!("raw_output={output}\n"));
        if output != "KEEP [REDACTED] END" {
            failures.push((spelling.to_string(), 99));
        }
    }
    record_output("json_control_spellings_respect_escape_boundaries", &records);
    assert!(failures.is_empty(), "escape boundary cases: {failures:?}");
}

#[test]
fn rejected_json_spelling_does_not_hide_raw_credential() {
    let mut redactor = ErrorRedactor::new("http://api.example/", "\tSECOND");
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-api-key",
        reqwest::header::HeaderValue::from_static("SECOND"),
    );
    redactor.headers(&headers);
    let input = r#"{"message":"\\tSECOND"}"#;
    let output = redactor.diagnostic(input);
    record_output(
        "rejected_json_spelling_does_not_hide_raw_credential",
        &output,
    );
    assert_eq!(output, r#"{"message":"\\t[REDACTED]"}"#);
    assert!(serde_json::from_str::<serde_json::Value>(&output).is_ok());
}

#[test]
fn common_request_url_wrappers_preserve_endpoint() {
    let url = "http://secret.example/secret?alt=sse&key=secret";
    let redactor = ErrorRedactor::new(url, "secret");
    let clean = "http://secret.example/secret?alt=[REDACTED]&key=[REDACTED]";
    let wrappers = [
        ("<", ">"),
        ("(", ")"),
        ("[", "]"),
        ("{", "}"),
        ("\"", "\""),
        ("'", "'"),
        ("`", "`"),
        (r#"\""#, r#"\""#),
    ];
    let mut records = String::new();
    let mut failures = vec![];
    for (left, right) in wrappers {
        let input = format!("endpoint {left}{url}{right} denied secret");
        let expected = format!("endpoint {left}{clean}{right} denied [REDACTED]");
        let output = redactor.diagnostic(&input);
        records.push_str(&format!(
            "wrapper={left}...{right}\ninput={input}\noutput={output}\nexpected={expected}\n"
        ));
        if output != expected {
            failures.push((left, right));
        }
    }
    record_output("common_request_url_wrappers_preserve_endpoint", &records);
    assert!(failures.is_empty(), "wrapper cases: {failures:?}");
}

#[test]
fn nested_request_url_boundary_search() {
    let url = "http://api.example/v1?alt=sse&key=TOKEN-SECRET";
    let redactor = ErrorRedactor::new(url, "TOKEN-SECRET");
    redactor.diagnostic("warm up");
    let repeats: usize = std::env::var("XIAOO_NESTED_REPEATS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let rounds: usize = std::env::var("XIAOO_NESTED_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let input = format!(
        "https://docs.example/help?lang=zh{}",
        format!("&next={url}").repeat(repeats)
    );
    let expected = input.replace("TOKEN-SECRET", "[REDACTED]");
    let mut times = vec![];
    for _ in 0..rounds {
        let start = Instant::now();
        let output = redactor.diagnostic(&input);
        times.push(start.elapsed().as_nanos());
        assert_eq!(output, expected);
        assert_eq!(output.matches("alt=sse").count(), repeats);
        assert_eq!(output.matches("[REDACTED]").count(), repeats);
    }
    record_output("nested_request_url_boundary_search", &format!("repeats={repeats} bytes={} rounds={rounds} elapsed_ns={times:?} expected_output=true\n", input.len()));
}
