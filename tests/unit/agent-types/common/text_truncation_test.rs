use super::*;

#[test]
fn truncate_keeps_text_within_cap_verbatim() {
    assert_eq!(truncate_scalars("hello", 10), "hello");
    assert_eq!(truncate_scalars("exact", 5), "exact");
    assert_eq!(truncate_scalars("", 5), "");
}

#[test]
fn truncate_appends_ellipsis_counted_into_cap() {
    assert_eq!(truncate_scalars("abcdef", 4), "abc…");
    assert_eq!(truncate_scalars("ab", 1), "…");
}

#[test]
fn truncate_counts_unicode_scalars_not_bytes() {
    // 5 个标量 15 字节；cap 4 → 前 3 个标量 + 省略号
    assert_eq!(truncate_scalars("你好世界吗", 4), "你好世…");
}

#[test]
fn truncate_zero_cap_yields_empty_string() {
    assert_eq!(truncate_scalars("anything", 0), "");
}
