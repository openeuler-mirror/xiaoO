/// Tests for the truncation layer:
/// - Byte + line double threshold (whichever triggers first)
/// - byte-level fallback for single long lines
/// - short-circuit avoiding O(n) line scan on oversized outputs
mod truncate_tool_tests {
    use super::*;

    /// Output under both the byte and line thresholds is returned
    /// as-is, no hint appended, no disk write attempted.
    #[test]
    fn small_output_passes_through_unchanged() {
        let out = truncate_tool_output("test_tool", "call_1", "hello world\n");
        assert_eq!(out, "hello world\n");
        assert!(
            !out.contains("[Tool output truncated"),
            "no truncation marker should appear for small output"
        );
    }

    /// Output that fits in bytes but exceeds the line threshold
    /// triggers truncation with a "line limit" trigger label, and
    /// the hint reports both byte and line counts.
    #[test]
    fn many_short_lines_triggers_line_limit_truncation() {
        // 3000 short lines × ~3 bytes = ~9 KiB total — under the
        // 50 KiB byte threshold but over the 2000-line threshold.
        let mut input = String::new();
        for i in 0..3_000 {
            input.push_str(&format!("l{i}\n"));
        }
        assert!(input.len() < MAX_TOOL_OUTPUT_BYTES);
        assert!(input.lines().count() > MAX_TOOL_OUTPUT_LINES);

        let out = truncate_tool_output("test_tool", "call_2", &input);
        assert!(
            out.contains("[Tool output truncated (line limit):"),
            "should report line-limit as the trigger; got: {out}"
        );
        assert!(
            out.contains("showing first"),
            "should report what was kept; got: {out}"
        );
        // The preview should contain the first 2000 lines, not all
        // 3000.
        let preview_line_count = out
            .split("\n[Tool output truncated")
            .next()
            .unwrap_or("")
            .lines()
            .count();
        assert_eq!(
            preview_line_count, MAX_TOOL_OUTPUT_LINES,
            "preview should contain exactly MAX_TOOL_OUTPUT_LINES lines"
        );
    }

    /// Output that fits in lines but exceeds the byte threshold
    /// triggers truncation with a "byte limit" trigger label.
    #[test]
    fn few_long_lines_triggers_byte_limit_truncation() {
        // 10 lines × 10 KiB each = 100 KiB total — over the 50 KiB
        // byte threshold but well under the 2000-line threshold.
        let long_line = "x".repeat(10 * 1024);
        let mut input = String::new();
        for _ in 0..10 {
            input.push_str(&long_line);
            input.push('\n');
        }
        assert!(input.lines().count() < MAX_TOOL_OUTPUT_LINES);
        assert!(input.len() > MAX_TOOL_OUTPUT_BYTES);

        let out = truncate_tool_output("test_tool", "call_3", &input);
        assert!(
            out.contains("[Tool output truncated (byte limit):"),
            "should report byte-limit as the trigger; got: {out}"
        );
    }

    /// `char_boundary_before` walks back to a UTF-8 char start,
    /// so truncating a string with multibyte chars doesn't split
    /// a codepoint in half. Sanity check on a small example.
    #[test]
    fn char_boundary_before_handles_multibyte_chars() {
        // "héllo" — 'é' is 2 bytes (0xC3 0xA9).
        let s = "héllo";
        // Index 2 lands inside 'é' (between byte 1 = 'h' and byte 2
        // = first byte of 'é'). The function should walk back to
        // 1 (the end of 'h').
        assert_eq!(char_boundary_before(s, 2), 1);
        // Index 3 is past 'é', so it stays at 3 (start of 'l').
        assert_eq!(char_boundary_before(s, 3), 3);
        // Past-the-end clamps to len.
        assert_eq!(char_boundary_before(s, 100), s.len());
    }

    /// A single line longer than `MAX_TOOL_OUTPUT_BYTES` (e.g. a
    /// minified bundle) must still produce a non-empty preview via
    /// the byte-level fallback. Without the fallback, the line-based
    /// loop would break on the first line (it exceeds the byte
    /// threshold) and leave `preview` empty — a regression from the
    /// old char-boundary truncation that always showed up to 50 KiB.
    #[test]
    fn single_long_line_produces_byte_level_preview() {
        // One line, 100 KiB — well over the 50 KiB byte threshold.
        let input = "x".repeat(100 * 1024);
        assert_eq!(input.lines().count(), 1);

        let out = truncate_tool_output("test_tool", "call_single", &input);
        // Must contain a truncation marker with "byte limit" trigger.
        assert!(
            out.contains("[Tool output truncated (byte limit):"),
            "should report byte-limit; got: {out}"
        );
        // The preview must NOT be empty — the byte-level fallback
        // should have produced a prefix.
        let preview = out.split("\n[Tool output truncated").next().unwrap_or("");
        assert!(
            !preview.is_empty(),
            "byte-level fallback must produce a non-empty preview; got empty"
        );
        // Preview should be <= MAX_TOOL_OUTPUT_BYTES (char-safe).
        assert!(
            preview.len() <= MAX_TOOL_OUTPUT_BYTES,
            "preview must be <= MAX_TOOL_OUTPUT_BYTES ({}), got {}",
            MAX_TOOL_OUTPUT_BYTES,
            preview.len()
        );
        // Preview should be > 0 — the whole point of the fallback.
        assert!(
            preview.len() > 0,
            "preview must have content, not just the hint"
        );
    }

    /// Verify the short-circuit: when `total_bytes > MAX_TOOL_OUTPUT_BYTES`,
    /// the fast path must NOT scan all lines (O(n)). We can't directly
    /// measure line-scan cost in a unit test, but we can verify the
    /// output is correct — a large byte-exceeding input is truncated
    /// with the right trigger and counts.
    #[test]
    fn oversized_bytes_truncates_without_full_line_scan() {
        // 10_000 lines × 100 bytes = ~1 MB — over the 50 KiB byte
        // threshold. Line count (10_000) also exceeds the 2000-line
        // threshold, but the byte threshold triggers first.
        let mut input = String::new();
        for i in 0..10_000 {
            input.push_str(&format!("{i:094}\n")); // ~95 bytes per line
        }
        assert!(input.len() > MAX_TOOL_OUTPUT_BYTES);

        let out = truncate_tool_output("test_tool", "call_oversized", &input);
        // Should be truncated — either byte or line limit triggers.
        assert!(
            out.contains("[Tool output truncated"),
            "should be truncated; got: {out}"
        );
        // Preview should be bounded.
        let preview = out.split("\n[Tool output truncated").next().unwrap_or("");
        assert!(
            preview.len() <= MAX_TOOL_OUTPUT_BYTES,
            "preview must be <= MAX_TOOL_OUTPUT_BYTES, got {}",
            preview.len()
        );
    }

    /// Multibyte char safety in the byte-level fallback: a single
    /// line whose byte-level cut point lands inside a multibyte
    /// sequence must not split a codepoint.
    #[test]
    fn byte_level_fallback_respects_utf8_boundary() {
        // Build a single line: ASCII prefix + multibyte chars that
        // straddle the MAX_TOOL_OUTPUT_BYTES boundary.
        // 'é' is 2 bytes (0xC3 0xA9). Fill up to just under the
        // threshold, then add multibyte chars so the boundary cut
        // lands inside one.
        let prefix = "a".repeat(MAX_TOOL_OUTPUT_BYTES - 1);
        // Append enough 'é' to push past the threshold.
        let input = format!("{prefix}ééééé");
        assert_eq!(input.lines().count(), 1);

        let out = truncate_tool_output("test_tool", "call_mb", &input);
        let preview = out.split("\n[Tool output truncated").next().unwrap_or("");
        // Must be valid UTF-8 (no panic, no broken codepoint).
        assert!(
            std::str::from_utf8(preview.as_bytes()).is_ok(),
            "preview must be valid UTF-8; got broken bytes"
        );
        // Must be <= MAX_TOOL_OUTPUT_BYTES.
        assert!(
            preview.len() <= MAX_TOOL_OUTPUT_BYTES,
            "preview must be <= MAX_TOOL_OUTPUT_BYTES, got {}",
            preview.len()
        );
    }
}
