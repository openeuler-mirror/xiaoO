pub use process_coordination::{spawn_std, spawn_std_blocking, write_permit_blocking};

/// Preserve status-style inherited streams, releasing coordination before wait.
pub async fn status(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    spawn_std(command).await?.wait()
}

/// Synchronous status-style creation; the child runs outside the permit.
pub fn status_blocking(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    spawn_std_blocking(command)?.wait()
}

/// Preserve output-style stream defaults, releasing coordination before wait.
pub fn output_blocking(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Output> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    spawn_std_blocking(command)?.wait_with_output()
}

#[cfg(test)]
#[path = "../../../tests/unit/shared/process_support_test.rs"]
mod tests;
