use std::sync::OnceLock;

use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use crate::input::{Input, InputRequest};

pub fn scroll_offset_from_drag(rel_y: usize, track_height: usize, max_scroll: usize) -> usize {
    if track_height <= 1 {
        return max_scroll;
    }
    let denominator = track_height.saturating_sub(1);
    max_scroll.saturating_mul(rel_y) / denominator
}

pub fn paste_into_input(input: &mut Input, text: &str) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for ch in normalized.chars() {
        input.handle(InputRequest::InsertChar(ch));
    }
}

pub(crate) fn line_prefix_width(line: &str, col_chars: usize) -> usize {
    line.chars()
        .take(col_chars)
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn use_ascii_terminal_symbols() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        cfg!(windows)
            || std::env::var_os("WT_SESSION").is_some()
            || std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::env::var_os("ConEmuPID").is_some()
    })
}

pub(crate) fn sanitize_terminal_text(text: &str) -> String {
    sanitize_terminal_text_for_mode(text, use_ascii_terminal_symbols())
}

fn sanitize_terminal_text_for_mode(text: &str, ascii_mode: bool) -> String {
    // Control characters must never reach the terminal as raw bytes: a
    // literal tab in a rendered cell is printed as 0x09, which the terminal
    // interprets as a tab-stop jump, desynchronizing every following
    // character from ratatui's 1-column advance and leaving the skipped
    // columns unrepainted (stale rows that only the next full repaint
    // clears). Tabs expand to four spaces; other control characters are
    // dropped. '\n' is kept: callers render line-by-line and ratatui splits
    // Paragraph text on it.
    let has_control = text.chars().any(|ch| ch.is_control() && ch != '\n');
    if !ascii_mode && !has_control {
        return text.to_string();
    }

    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\t' => output.push_str("    "),
            '\n' => output.push('\n'),
            _ if ch.is_control() => {}
            _ if !ascii_mode => output.push(ch),
            '▎' | '▌' | '│' => output.push('|'),
            '▾' => output.push('v'),
            '▸' => output.push('>'),
            '⟡' => output.push('*'),
            '⭕' => output.push('o'),
            '◔' => output.push_str("[-]"),
            '•' => output.push('*'),
            '●' | '○' => output.push('o'),
            '✅' => output.push_str("[x]"),
            '✓' => output.push('x'),
            '☐' | '□' => output.push_str("[ ]"),
            '→' => output.push_str("->"),
            '←' => output.push_str("<-"),
            '—' | '–' => output.push('-'),
            '…' => output.push_str("..."),
            '─' => output.push('-'),
            // Scroll-affordance arrows on the wide-table hint line.
            '◀' => output.push('<'),
            '▶' => output.push('>'),
            '⠋' | '⠼' | '⠇' => output.push('|'),
            '⠙' | '⠴' | '⠏' => output.push('/'),
            '⠹' | '⠦' => output.push('-'),
            '⠸' | '⠧' => output.push('\\'),
            _ => output.push(ch),
        }
    }
    output
}

pub(crate) fn truncate_display_width(text: &str, max_width: usize) -> String {
    truncate_display_width_for_mode(text, max_width, use_ascii_terminal_symbols())
}

pub(crate) fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn truncate_display_width_for_mode(text: &str, max_width: usize, ascii_mode: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    let text = sanitize_terminal_text_for_mode(text, ascii_mode);
    let ellipsis = sanitize_terminal_text_for_mode("…", ascii_mode);
    let total_width = display_width(&text);
    if total_width <= max_width {
        return text;
    }

    let ellipsis_width = display_width(&ellipsis);
    if ellipsis_width >= max_width {
        let mut output = String::new();
        let mut used = 0usize;
        for ch in text.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + width > max_width {
                break;
            }
            output.push(ch);
            used += width;
        }
        return output;
    }

    let keep_width = max_width.saturating_sub(ellipsis_width);
    let mut output = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > keep_width {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push_str(&ellipsis);
    output
}

pub(crate) fn render_tool_detail_text(text: &str) -> String {
    text.replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\r", "\n")
        .replace("\\t", "\t")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

/// Find the first occurrence of `needle` within `haystack` starting from the
/// `start`-th char. Returns the char index where the match begins. Uses
/// `str::find` (which respects UTF-8 char boundaries for valid UTF-8 needles)
/// so no `Vec<char>` allocation is needed.
///
/// Used to reverse-map a wrapped visual line's text back to its position in
/// the original logical line: textwrap strips inter-word whitespace at wrap
/// boundaries, so each visual line's text is a contiguous substring of the
/// original but not necessarily at a contiguous offset. Advancing via this
/// helper skips the stripped whitespace and lands on the correct char offset.
pub(crate) fn find_substring_from(haystack: &str, needle: &str, start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start);
    }
    let start_byte = haystack
        .char_indices()
        .nth(start)
        .map_or(haystack.len(), |(b, _)| b);
    let byte_pos = haystack[start_byte..].find(needle)?;
    Some(haystack[..start_byte + byte_pos].chars().count())
}

/// Fit a popup of the preferred `width` x `height` into `bounds`, centred,
/// never exceeding `bounds`.
///
/// Popup geometry is written as a preferred size — `area.width.clamp(min,
/// max)` — and then centred in the frame. The clamp's *lower* bound is what
/// makes a narrow or short terminal dangerous: the popup claims more columns
/// or rows than the frame has, so `x + width` / `y + height` reach past the
/// buffer.
///
/// `Block` and `List` survive an oversized rect because they intersect it with
/// the buffer before writing (`Buffer::set_style` does the same). `Clear` and
/// `Paragraph` do not: `Clear::render_ref` iterates `buf[(x, y)]` over the raw
/// area, and `Paragraph::render_text` writes one cell per grapheme at
/// `buf[(area.left() + x, area.top() + y)]`. Both go through
/// `Buffer::index_of`, which panics once the index falls outside the frame —
/// the narrow-terminal crash this helper exists to prevent.
///
/// This is the one place that turns a *preferred* size into a rect, so a new
/// renderer cannot forget the clamp: build the preferred size, hand it here, and
/// every area derived from the result (block inner, `Layout` chunks, hand-built
/// hint rows) stays inside the frame. `Rect::clamp` in ratatui does the same
/// clamp-and-move for an already-built rect; this helper additionally *centres*
/// before clamping, which is what every popup here wants, so the two stay split:
/// `Rect::clamp` would be worth reaching for if a caller ever wants a popup that
/// is not centred.
///
/// Clamping the popup rect to the frame here keeps every area derived from it
/// (block inner, `Layout` chunks, hint rows) inside the frame, so a narrow or
/// short terminal shrinks the dialog instead of crashing the TUI.
pub(crate) fn fit_popup_rect(bounds: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(bounds.width);
    let height = height.min(bounds.height);
    Rect {
        x: bounds.x + (bounds.width.saturating_sub(width)) / 2,
        y: bounds.y + (bounds.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// Whether `area` is non-empty and fully inside `frame`.
///
/// Areas derived from a clamped popup rect satisfy this by construction
/// (`Layout` splits stay inside their input, `Block::inner` shrinks to nothing
/// when the box is too small). Rows a renderer *builds by hand* — a hint pinned
/// to the last row of the box, a button pinned to a row of an input chunk — do
/// not: when the popup has shrunk to one or two rows the block inner collapses
/// to height 0 while such a row keeps `height: 1`, landing one row below the
/// dialog and outside the frame. Since `Clear`/`Paragraph` do not clip, those
/// rows must be checked before rendering.
pub(crate) fn fits_in_frame(frame: Rect, area: Rect) -> bool {
    !area.is_empty()
        && area.x >= frame.x
        && area.y >= frame.y
        && area.right() <= frame.right()
        && area.bottom() <= frame.bottom()
}

/// Split a popup's inner area into the list body and the optional hint row.
///
/// The hint is the least important row, so a box that shrank to a single row
/// drops it -- and the row it would have occupied goes back to the list instead
/// of being left blank. Deciding both things in one place is the point: when the
/// gate and the reserved row were computed separately, a 3-row terminal produced
/// `inner.height - 1 == 0` list rows while the hint was gated off, i.e. a
/// border-only popup whose entries were still selectable with the keyboard but
/// invisible on screen.
///
/// `None` means "do not draw a hint" and the list owns the whole inner area.
pub(crate) fn split_hint_row(inner: Rect) -> (Rect, Option<Rect>) {
    if inner.height < 2 {
        return (inner, None);
    }
    let list = Rect {
        height: inner.height - 1,
        ..inner
    };
    let hint = Rect {
        y: inner.y + inner.height - 1,
        height: 1,
        ..inner
    };
    (list, Some(hint))
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/render/utils_test.rs"]
mod tests;
