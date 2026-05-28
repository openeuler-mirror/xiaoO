use std::error::Error;
use std::time::Duration;

pub fn format_reqwest_error(e: reqwest::Error, context: &str) -> String {
    let mut parts: Vec<String> = Vec::new();

    if e.is_timeout() {
        parts.push(format!("{}: request timed out", context));
    } else if e.is_connect() {
        parts.push(format!("{}: connection failed", context));
    } else {
        parts.push(format!("{}: request failed", context));
    }

    parts.push(format!("  reqwest: {}", e));

    let mut source = e.source();
    let mut depth = 0;
    while let Some(src) = source {
        depth += 1;
        parts.push(format!("  cause {}: {}", depth, src));
        source = src.source();
    }

    parts.join("\n")
}

pub fn build_http_client(timeout_ms: u64) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms));

    let native_certs = rustls_native_certs::load_native_certs();
    for cert in native_certs.certs {
        if let Ok(c) = reqwest::tls::Certificate::from_der(cert.as_ref()) {
            builder = builder.add_root_certificate(c);
        }
    }

    builder
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}
