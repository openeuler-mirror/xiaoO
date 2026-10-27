use std::fs::{self, OpenOptions};
use std::io::Write;

use xiaoo_shared::error_log_path;

const MAX_SUMMARY_CHARS: usize = 220;

pub(crate) fn record_tui_error(source: &str, error: impl AsRef<str>) -> String {
    let error = error.as_ref();
    let summary = visible_error_summary(error);
    // The hint must resolve the same way as the write itself: a hardcoded
    // default would misdirect operators whenever XIAOO_ERROR_LOG redirects
    // the file (e.g. to a container volume).
    let display_path = error_log_path().display().to_string();
    let log_status = match append_error_log(source, error) {
        Ok(()) => format!("错误已写入 {display_path}。"),
        Err(write_error) => format!("错误写入 {display_path} 失败: {write_error}"),
    };

    if summary.is_empty() {
        format!("Error: 操作失败。\n{log_status}")
    } else {
        format!("Error: {summary}\n{log_status}")
    }
}

/// Append a raw entry to the shared error log resolved by
/// [`agent_types::common::error_log_path`] (re-exported through
/// `xiaoo_shared`). Exposed so non-error diagnostics (e.g.
/// `on_tool_completed` parameter dumps) can share the same log file
/// without going through the user-facing `record_tui_error`.
pub(crate) fn append_error_log(source: &str, error: &str) -> std::io::Result<()> {
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
#[path = "../../../../tests/unit/endside/support/error_log_test.rs"]
mod tests;
