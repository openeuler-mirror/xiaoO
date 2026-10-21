use super::current_time_ms;

#[test]
fn current_time_ms_is_reasonable_epoch_value() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    let now = current_time_ms();
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0);
    assert!(now >= before);
    assert!(now <= after);
}

#[test]
fn current_time_ms_is_monotonic_across_calls() {
    let first = current_time_ms();
    let second = current_time_ms();
    assert!(second >= first);
}
