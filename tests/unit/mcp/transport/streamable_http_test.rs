use super::{
    overall_deadline, remaining, retry_after_delay, DEFAULT_INITIALIZE_RATE_LIMIT_RETRY_DELAY,
};
use reqwest::header::{HeaderMap, HeaderValue};
use std::time::Duration;
use tokio::time::Instant;

#[test]
fn non_initialize_request_uses_the_full_configured_deadline() {
    let timeout = Duration::from_millis(1_000);
    let earliest = Instant::now() + timeout;
    let request_deadline = overall_deadline(1_000);
    let latest = Instant::now() + timeout;

    assert!(request_deadline >= earliest);
    assert!(request_deadline <= latest);
}

#[test]
fn rate_limit_retry_delay_uses_retry_after_seconds_or_a_small_fallback() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("2"));
    assert_eq!(retry_after_delay(&headers), Duration::from_secs(2));

    headers.insert("retry-after", HeaderValue::from_static("invalid"));
    assert_eq!(
        retry_after_delay(&headers),
        DEFAULT_INITIALIZE_RATE_LIMIT_RETRY_DELAY
    );
}

#[test]
fn elapsed_deadline_has_no_remaining_grace_period() {
    let deadline = Instant::now() - Duration::from_millis(1);

    assert_eq!(remaining(deadline), Duration::ZERO);
}
