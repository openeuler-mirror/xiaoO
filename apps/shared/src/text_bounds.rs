//! UTF-8 char-boundary helpers for text that is truncated or spliced by byte
//! counts.
//!
//! `str` slicing requires the index to fall on a char boundary. An index
//! derived from `String::len()` or from a byte-wise prefix scan can land
//! inside a multi-byte character (3 bytes for CJK, 4 for emoji), and
//! `&s[..index]` then panics with `byte index N is not a char boundary`.
//!
//! These helpers are the single place that converts such an index into a
//! safe one, so daemon-side preview formatting and TUI-side completion agree
//! on the same rule.

/// Largest byte index `<= index` that lies on a UTF-8 char boundary, clamped
/// to `s.len()`.
///
/// Equivalent to `str::floor_char_boundary` (stable since Rust 1.80) but
/// written inline so it compiles on any Rust version the CI may pin — the
/// same rationale as `xiaoo_core`'s `char_boundary_before`.
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let bytes = s.as_bytes();
    let mut boundary = index;
    // Continuation bytes are `0b10xx_xxxx`; walk back to the leading byte.
    while boundary > 0 && bytes[boundary] & 0xC0 == 0x80 {
        boundary -= 1;
    }
    boundary
}

/// Longest prefix of `s` that is at most `max_bytes` long and ends on a char
/// boundary.
///
/// Never panics, for any `max_bytes`.
pub fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    &s[..floor_char_boundary(s, max_bytes)]
}

#[cfg(test)]
#[path = "../../../tests/unit/shared/text_bounds_test.rs"]
mod tests;
