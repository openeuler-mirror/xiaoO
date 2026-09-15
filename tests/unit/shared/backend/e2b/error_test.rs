use super::*;

#[test]
fn retryable_statuses_are_classified() {
    assert!(E2bFailure::from_status("create", StatusCode::TOO_MANY_REQUESTS, "".into()).retryable);
    assert!(E2bFailure::from_status("create", StatusCode::BAD_GATEWAY, "".into()).retryable);
    assert!(!E2bFailure::from_status("create", StatusCode::UNAUTHORIZED, "".into()).retryable);
}
