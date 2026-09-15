use super::*;

#[test]
fn parse_valid_5_field() {
    let expr = CronExpression::parse("0 9 * * mon-fri").unwrap();
    // 5-field gets normalized to 6-field with "0 " prefix for seconds
    assert_eq!(expr.as_str(), "0 0 9 * * mon-fri");
}

#[test]
fn parse_valid_6_field_with_seconds() {
    let expr = CronExpression::parse("30 0 9 * * mon-fri").unwrap();
    assert_eq!(expr.as_str(), "30 0 9 * * mon-fri");
}

#[test]
fn parse_empty_returns_error() {
    let err = CronExpression::parse("").unwrap_err();
    assert!(matches!(err, CronParseError::Empty));
}

#[test]
fn parse_whitespace_only_returns_error() {
    let err = CronExpression::parse("   ").unwrap_err();
    assert!(matches!(err, CronParseError::Empty));
}

#[test]
fn parse_invalid_returns_error() {
    let err = CronExpression::parse("not a cron expression").unwrap_err();
    assert!(matches!(err, CronParseError::InvalidSyntax { .. }));
}

#[test]
fn parse_invalid_field_count_returns_error() {
    let err = CronExpression::parse("* * *").unwrap_err();
    assert!(matches!(err, CronParseError::InvalidSyntax { .. }));
}

#[test]
fn next_after_returns_future_time() {
    let expr = CronExpression::parse("0 9 * * *").unwrap();
    let now = chrono::Utc::now();
    let next = expr.next_after(now);
    assert!(next.is_some());
    assert!(next.unwrap() > now);
}

#[test]
fn next_after_step_expression() {
    let expr = CronExpression::parse("*/5 * * * *").unwrap();
    let now = chrono::Utc::now();
    let next = expr.next_after(now).unwrap();
    // Next trigger should be within the next 5 minutes
    let diff_minutes = (next - now).num_minutes();
    assert!(diff_minutes >= 0 && diff_minutes <= 5);
}

#[test]
fn display_returns_normalized_string() {
    let expr = CronExpression::parse("0 9 * * mon-fri").unwrap();
    assert_eq!(format!("{}", expr), "0 0 9 * * mon-fri");
}
