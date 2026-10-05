use super::*;

#[test]
fn every_comma_separated_segment_counts_towards_the_limit() {
    // `"1,1-4294967295"` used to pass validation: only the first segment
    // was measured, while the reader expanded the whole string and looped
    // from 1 to u32::MAX.
    let result = validate_pages("1,1-4294967295");
    assert!(!result.result, "a huge second segment must be rejected");
    assert_eq!(result.error_code, Some(error_code::PAGES_RANGE_TOO_LARGE));

    // Same for a segment that arrives after a valid one.
    assert!(!validate_pages("1-10,1-100000").result);

    // Contrast: a string whose segments are each small stays valid.
    assert!(validate_pages("1-10,20,30-40").result);
    assert!(validate_pages("1,2,3").result);
    assert!(validate_pages("1-100").result);

    // Format validation is unchanged.
    assert_eq!(
        validate_pages("0-5").error_code,
        Some(error_code::INVALID_PAGES_FORMAT)
    );
    assert_eq!(
        validate_pages("5-1").error_code,
        Some(error_code::INVALID_PAGES_FORMAT)
    );
}

#[test]
fn total_pages_sums_across_segments() {
    assert_eq!(total_pages("1").unwrap(), 1);
    assert_eq!(total_pages("1-10").unwrap(), 10);
    assert_eq!(total_pages("1-10,20").unwrap(), 11);
    assert_eq!(total_pages("1-10,20-29").unwrap(), 20);
    // Overlapping segments are counted twice on purpose: the check may
    // reject earlier than strictly necessary, never later.
    assert_eq!(total_pages("1-10,5-14").unwrap(), 20);
    assert!(total_pages("").is_err());
    assert!(total_pages("1,,2").is_err());
}
