//! Output rendering utilities.

use crate::app::storage::ExecutionRecord;

/// Render execution records as a table.
pub fn render_history(records: &[ExecutionRecord]) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "{:<8} {:<10} {:<8} {}\n",
        "ID", "EXIT", "TIME", "COMMAND"
    ));
    output.push_str(&"-".repeat(60));
    output.push('\n');

    for record in records {
        let status = if record.exit_code == 0 {
            "OK"
        } else if record.timed_out {
            "TIMEOUT"
        } else {
            "FAIL"
        };

        let time = format!("{}ms", record.duration_ms);
        let cmd = if record.command.len() > 40 {
            format!("{}...", &record.command[..37])
        } else {
            record.command.clone()
        };

        output.push_str(&format!(
            "{:<8} {:<10} {:<8} {}\n",
            record.id, status, time, cmd
        ));
    }

    output
}

/// Render a single execution result.
pub fn render_result(result: &cerberus_core::ExecResult) -> String {
    let mut output = String::new();

    output.push_str(&format!("Exit Code: {}\n", result.exit_code));

    if !result.stdout.is_empty() {
        output.push_str("\nStdout:\n");
        output.push_str(&String::from_utf8_lossy(&result.stdout));
    }

    if !result.stderr.is_empty() {
        output.push_str("\nStderr:\n");
        output.push_str(&String::from_utf8_lossy(&result.stderr));
    }

    output
}
