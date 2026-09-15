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
