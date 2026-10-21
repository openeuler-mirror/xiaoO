/// Milliseconds elapsed since the UNIX epoch.
///
/// Saturates to `0` when the system clock is set before 1970-01-01
/// instead of panicking, so callers can treat timestamps as plain
/// monotonically-ordered values.
pub fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-types/common/time_test.rs"]
mod tests;
