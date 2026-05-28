use std::error::Error;

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
