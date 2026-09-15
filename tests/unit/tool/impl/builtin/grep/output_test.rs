use super::*;

/// `with_limit` (called when the result was capped by `apply_head_limit`)
/// must set `partial = true` so callers don't mistake aggregate fields
/// for totals. This is the fix for the count-mode `total_matches`
/// regression: a capped count result now carries an explicit signal
/// that `num_matches` is a partial sum across the shown subset only.
#[test]
fn with_limit_sets_partial_flag() {
    let output = GrepOutput::new(OutputMode::Count)
        .with_count(1250, 250, "file_a:5\nfile_b:3".to_string())
        .with_limit(250);
    assert!(output.partial, "partial must be true after with_limit");
    assert_eq!(
        output.applied_limit,
        Some(250),
        "applied_limit must be set together with partial"
    );
}

/// Without `with_limit`, `partial` stays false — aggregate fields
/// represent the complete result and callers may treat them as totals.
#[test]
fn partial_defaults_to_false_when_not_capped() {
    let output =
        GrepOutput::new(OutputMode::Count).with_count(42, 3, "file_a:5\nfile_b:37".to_string());
    assert!(
        !output.partial,
        "partial must be false when the result was not capped"
    );
    assert!(output.applied_limit.is_none());
}

/// `partial` serializes only when true (`skip_serializing_if = is_false`
/// — absent in JSON when false, present when true). Backwards-compat
/// for consumers that don't know about the field: they still parse
/// the JSON fine; consumers that DO know can read the explicit signal.
#[test]
fn partial_serializes_only_when_true() {
    // Capped result → JSON includes "partial": true.
    let capped = GrepOutput::new(OutputMode::Content)
        .with_content("line1\nline2".to_string(), 2)
        .with_limit(2);
    let capped_json = serde_json::to_value(&capped).unwrap();
    assert_eq!(capped_json["partial"], serde_json::Value::Bool(true));

    // Uncapped result → JSON omits "partial" entirely (default false).
    let uncapped = GrepOutput::new(OutputMode::Content).with_content("line1\nline2".to_string(), 2);
    let uncapped_json = serde_json::to_value(&uncapped).unwrap();
    assert!(
        uncapped_json.get("partial").is_none(),
        "partial must be omitted from JSON when false; got {:?}",
        uncapped_json.get("partial")
    );
}
