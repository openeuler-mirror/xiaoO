use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const DISPLAY_LOG_PATH: &str = "~/.xiaoo/log/error.log";
const MAX_SUMMARY_CHARS: usize = 220;

pub(crate) fn record_tui_error(source: &str, error: impl AsRef<str>) -> String {
    let error = error.as_ref();
    let summary = visible_error_summary(error);
    let log_status = match append_error_log(source, error) {
        Ok(()) => format!("错误已写入 {DISPLAY_LOG_PATH}。"),
        Err(write_error) => format!("错误写入 {DISPLAY_LOG_PATH} 失败: {write_error}"),
    };

    if summary.is_empty() {
        format!("Error: 操作失败。\n{log_status}")
    } else {
        format!("Error: {summary}\n{log_status}")
    }
}

fn append_error_log(source: &str, error: &str) -> std::io::Result<()> {
    let path = error_log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let timestamp = chrono::Local::now().to_rfc3339();
    let source = source.split_whitespace().collect::<Vec<_>>().join("_");
    writeln!(file, "===== {timestamp} source={source} =====")?;
    writeln!(file, "{error}")?;
    writeln!(file)?;
    Ok(())
}

fn error_log_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".xiaoo")
        .join("log")
        .join("error.log")
}

fn visible_error_summary(error: &str) -> String {
    let first_line = error
        .lines()
        .next()
        .unwrap_or_default()
        .split("Request body:")
        .next()
        .unwrap_or_default();
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&compact, MAX_SUMMARY_CHARS)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::visible_error_summary;

    #[test]
    fn summary_strips_request_body() {
        let summary =
            visible_error_summary("API error: HTTP 502 Bad Gateway\nRequest body: secret prompt");
        assert_eq!(summary, "API error: HTTP 502 Bad Gateway");
    }

    #[test]
    fn summary_compacts_and_truncates() {
        let error = format!("failed   {}\nsecond line", "x".repeat(400));
        let summary = visible_error_summary(&error);
        assert!(summary.len() < error.len());
        assert!(summary.ends_with("..."));
        assert!(!summary.contains('\n'));
    }
}
