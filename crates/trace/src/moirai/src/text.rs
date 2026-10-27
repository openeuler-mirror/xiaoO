//! Display formatting shared by the `moirai` CLI and any library consumer.
//!
//! Truncation has to be UTF-8 safe: trace `extras` are JSON that routinely
//! carries Chinese task text, and both a byte index derived from `len()` and
//! a raw `&s[..max_len - 3]` slice can land inside a 3-byte character, which
//! panics with `byte index N is not a char boundary`.
//!
//! The char-boundary helper is inlined here rather than imported: `moirai` is
//! a leaf crate with no workspace dependencies, and it must stay that way so
//! the CLI builds standalone.

use serde_json::Value;

/// Largest byte index `<= index` that lies on a UTF-8 char boundary, clamped
/// to `s.len()`.
///
/// Equivalent to `str::floor_char_boundary` (stable since Rust 1.80) but
/// written inline so it compiles on any Rust version the CI may pin.
fn floor_char_boundary(s: &str, index: usize) -> usize {
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

/// Renders `json` compactly and truncates it to at most `max_bytes` bytes,
/// ending on a char boundary and appending `...` when anything was dropped.
///
/// ASCII input yields exactly the previous output (the budget includes the
/// three dots). The returned string is never longer than `max_bytes`, except
/// when `max_bytes < 3`, where the ellipsis alone does not fit.
pub fn truncate_json_for_display(json: &Value, max_bytes: usize) -> String {
    let rendered = json.to_string();
    if rendered.len() <= max_bytes {
        return rendered;
    }
    let cut = floor_char_boundary(&rendered, max_bytes.saturating_sub(3));
    format!("{}...", &rendered[..cut])
}

#[cfg(test)]
#[path = "../../../../../tests/unit/trace/moirai/text_test.rs"]
mod tests;
