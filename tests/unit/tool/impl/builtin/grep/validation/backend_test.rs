use super::*;

fn input_with_timeout(timeout: Option<u64>) -> GrepInput {
    GrepInput {
        pattern: "x".to_string(),
        timeout,
        ..Default::default()
    }
}

#[test]
fn timeout_none_is_ok() {
    assert!(validate_timeout(&input_with_timeout(None)).result);
}

#[test]
fn timeout_zero_is_invalid() {
    let result = validate_timeout(&input_with_timeout(Some(0)));
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::TIMEOUT_INVALID));
}

#[test]
fn timeout_above_max_is_rejected() {
    let result = validate_timeout(&input_with_timeout(Some(max_timeout_ms() + 1)));
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::TIMEOUT_EXCEEDS_MAX));
    assert!(result
        .message
        .as_ref()
        .is_some_and(|m| m.contains("exceeds maximum allowed")));
}

#[test]
fn timeout_at_max_is_ok() {
    assert!(validate_timeout(&input_with_timeout(Some(max_timeout_ms()))).result);
}

#[test]
fn validate_input_reaches_timeout_check_with_valid_pattern() {
    // Pattern/path are valid here, so the timeout branch must be reached.
    let result = validate_input(&input_with_timeout(Some(0)));
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::TIMEOUT_INVALID));
}

#[test]
fn validate_input_pattern_precedence_over_timeout() {
    // Empty pattern must win over a bad timeout so callers get the most
    // actionable error first.
    let result = validate_input(&GrepInput {
        pattern: "   ".to_string(),
        timeout: Some(0),
        ..Default::default()
    });
    assert!(!result.result);
    assert_eq!(result.error_code, Some(error_code::PATTERN_EMPTY));
}
