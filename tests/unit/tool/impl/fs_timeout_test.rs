use super::*;
use std::time::Duration;

#[tokio::test]
async fn fast_future_returns_value_unchanged() {
    let fut = async { Ok::<u32, &str>(42) };
    let result = timed("op", 1000, fut).await;
    assert_eq!(result, Ok(42));
}

#[tokio::test]
async fn inner_error_display_is_forwarded() {
    let fut = async { Err::<u32, String>("boom".to_string()) };
    let result = timed("op", 1000, fut).await;
    assert_eq!(result.unwrap_err(), "boom");
}

#[tokio::test]
async fn slow_future_times_out_with_label_and_duration() {
    // 5s sleep guarded by a 50ms budget — must hit the timeout branch.
    let fut = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok::<(), &str>(())
    };
    let result = timed("slow op", 50, fut).await;
    let err = result.unwrap_err();
    assert!(err.contains("slow op"), "missing label: {err}");
    assert!(err.contains("50ms"), "missing duration: {err}");
    assert!(err.contains("timed out"), "missing keyword: {err}");
}

#[tokio::test]
async fn inner_error_takes_precedence_over_implicit_deadline() {
    // Future resolves with an error well before the deadline: the error
    // path, not the timeout path, must be reported.
    let fut = async { Err::<u32, &str>("fail") };
    let result = timed("op", 10_000, fut).await;
    assert_eq!(result.unwrap_err(), "fail");
}
