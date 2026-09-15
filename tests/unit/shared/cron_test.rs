use super::validate_cron_expr;

#[test]
fn valid_expression_passes() {
    assert!(validate_cron_expr("0 * * * *").is_ok());
    assert!(validate_cron_expr("0 0 * * *").is_ok());
}

#[test]
fn empty_expression_rejected() {
    let err = validate_cron_expr("").expect_err("empty should fail");
    assert!(!err.is_empty());
}

#[test]
fn invalid_expression_rejected() {
    let err = validate_cron_expr("not a cron").expect_err("garbage should fail");
    assert!(!err.is_empty());
}
