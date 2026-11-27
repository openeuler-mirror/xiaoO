//! Redacted TOML error locations: raw messages may quote secrets.

/// Location suffix (" at line X column Y"), or empty; the raw message may quote secrets.
pub fn locate_toml_error(content: &str, error: &toml::de::Error) -> String {
    let Some(span) = error.span().filter(|span| !span.is_empty()) else {
        return String::new();
    };
    // Clamp to a char boundary: a mid-character offset would panic.
    let mut offset = span.start.min(content.len());
    while !content.is_char_boundary(offset) {
        offset = offset.saturating_sub(1);
    }
    let line = content[..offset].matches('\n').count() + 1;
    let line_start = content[..offset].rfind('\n').map_or(0, |pos| pos + 1);
    let column = offset - line_start + 1;
    format!(" at line {line} column {column}")
}
