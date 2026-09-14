use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::theme::Theme;
use super::utils::sanitize_terminal_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TableAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
struct MarkdownTable {
    header: Vec<String>,
    aligns: Vec<TableAlign>,
    rows: Vec<Vec<String>>,
}

/// A markdown table whose natural width exceeds the render viewport and
/// was therefore windowed to the display-column slice
/// `[horiz_offset, horiz_offset + viewport_width)` so every rendered line
/// stays within the viewport and cell text is never ellipsised.
///
/// Rendered by [`render_table`]; surfaced to the transcript layer via
/// [`render_markdown_with_horiz`] so wide tables can be panned horizontally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideTableRegion {
    /// Rendered logical-line index (0-based, within the markdown output)
    /// where this wide table begins.
    pub start_line: usize,
    /// Number of rendered logical lines the table occupies, including its
    /// trailing hint line.
    pub line_count: usize,
    /// Full natural display width of the table (terminal columns).
    pub natural_width: usize,
    /// Display-column window start used when rendering the table.
    pub horiz_offset: usize,
    /// Render viewport width (terminal columns) the window was clipped to.
    /// `natural_width - viewport_width` is the maximum `horiz_offset`.
    pub viewport_width: usize,
}

/// Mutable state carried across lines by the markdown line parser.
///
/// Only `in_code_block`, `code_language`, and `show_code_language_label`
/// cross line boundaries; every other construct (headings, HR, lists,
/// inline paragraphs) is purely per-line. This struct is the snapshot
/// captured at a "safe" freeze point for incremental streaming.
#[derive(Clone, Default)]
struct MarkdownParseState {
    in_code_block: bool,
    code_language: String,
    show_code_language_label: bool,
}

/// Output of [`parse_markdown_lines`]: the rendered lines plus the parser
/// state at the end of the consumed slice.
struct MarkdownParseOutcome {
    lines: Vec<Line<'static>>,
    state: MarkdownParseState,
    /// Windowed wide tables discovered while parsing, in render order.
    /// `start_line` is relative to `lines`.
    wide_tables: Vec<WideTableRegion>,
}

/// Core line-oriented markdown state machine, parameterised by the
/// incoming parser state. Consumes `raw_lines` from the start; returns
/// the rendered lines and the state at the end of the slice.
///
/// This is the single source of truth for line parsing — both
/// [`render_markdown`] (full, from an empty state) and
/// [`render_markdown_incremental`] (streaming, from a frozen state) call
/// into here. Tables are handled by the caller via a full-render fallback
/// (see [`render_markdown_incremental`]).
fn parse_markdown_lines(
    raw_lines: &[&str],
    mut state: MarkdownParseState,
    theme: &Theme,
    width: u16,
    table_horiz_offset: usize,
) -> MarkdownParseOutcome {
    let mut lines = Vec::new();
    let mut wide_tables = Vec::new();
    let mut line_index = 0;

    while line_index < raw_lines.len() {
        let raw_line = raw_lines[line_index];
        let trimmed = raw_line.trim();
        let trimmed_start = raw_line.trim_start();

        if trimmed_start.starts_with("```") {
            if state.in_code_block {
                state.in_code_block = false;
                state.code_language.clear();
                state.show_code_language_label = false;
            } else {
                state.in_code_block = true;
                state.code_language = trimmed_start.trim_start_matches("```").trim().to_string();
                state.show_code_language_label = !state.code_language.is_empty();
            }
            line_index += 1;
            continue;
        }

        if state.in_code_block {
            if state.show_code_language_label {
                let label_style = Style::default().fg(theme.muted).bg(theme.code_bg);
                lines.push(Line::from(vec![Span::styled(
                    sanitize_terminal_text(&format!("  {} ", state.code_language)),
                    label_style,
                )]));
                state.show_code_language_label = false;
            }

            let code_style = Style::default().fg(theme.code_fg).bg(theme.code_bg);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(&format!("  {raw_line}")),
                code_style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some((table, consumed)) = parse_table_block(&raw_lines[line_index..]) {
            let table_start = lines.len();
            let (table_lines, wide) = render_table(&table, theme, width, table_horiz_offset);
            lines.extend(table_lines);
            if let Some((natural_width, horiz_offset)) = wide {
                wide_tables.push(WideTableRegion {
                    start_line: table_start,
                    line_count: lines.len() - table_start,
                    natural_width,
                    horiz_offset,
                    viewport_width: width as usize,
                });
            }
            line_index += consumed;
            continue;
        }

        if trimmed.is_empty() {
            lines.push(Line::from(String::new()));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("### ") {
            let style = Style::default()
                .fg(theme.secondary)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(content),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("## ") {
            let style = Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(content),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start.strip_prefix("# ") {
            let style = Style::default()
                .fg(theme.accent)
                .bg(theme.background)
                .add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(content),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let hr_len = usize::max(1, width as usize);
            let style = Style::default().fg(theme.muted).bg(Color::Reset);
            lines.push(Line::from(vec![Span::styled(
                sanitize_terminal_text(&"─".repeat(hr_len)),
                style,
            )]));
            line_index += 1;
            continue;
        }

        if let Some(content) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
        {
            let mut spans = vec![Span::styled(
                sanitize_terminal_text("  • "),
                Style::default().fg(theme.secondary).bg(theme.background),
            )];
            spans.extend(parse_inline(content, theme).spans);
            lines.push(Line::from(spans));
            line_index += 1;
            continue;
        }

        if let Some((prefix, content)) = parse_numbered_prefix(trimmed_start) {
            let mut spans = vec![Span::styled(
                format!("  {prefix} "),
                Style::default().fg(theme.secondary).bg(theme.background),
            )];
            spans.extend(parse_inline(content, theme).spans);
            lines.push(Line::from(spans));
            line_index += 1;
            continue;
        }

        lines.push(parse_inline(raw_line, theme));
        line_index += 1;
    }

    MarkdownParseOutcome {
        lines,
        state,
        wide_tables,
    }
}

/// Retained as a compatibility wrapper with the original signature (window
/// start 0 — the leftmost window). Only exercised by tests now that callers
/// use [`render_markdown_with_horiz`] / [`render_markdown_incremental`].
#[allow(dead_code)]
pub fn render_markdown(text: &str, theme: &Theme, width: u16) -> Vec<Line<'static>> {
    render_markdown_with_horiz(text, theme, width, 0).0
}

/// Render markdown with an optional horizontal window for wide tables.
///
/// Wide tables (natural width > `width`) are rendered at their natural
/// column widths and then sliced to the display-column window
/// `[table_horiz_offset, table_horiz_offset + width)`; every returned
/// logical line stays within `width` columns and **no cell text is
/// ellipsised**. Narrow tables render exactly as before. The returned
/// `wide_tables` describe the windowed tables for the transcript layer to
/// make them horizontally scrollable.
///
/// [`render_markdown`] is a thin wrapper keeping its original signature
/// (window start 0 — the leftmost window), so existing callers and
/// regression tests are unaffected.
pub fn render_markdown_with_horiz(
    text: &str,
    theme: &Theme,
    width: u16,
    table_horiz_offset: usize,
) -> (Vec<Line<'static>>, Vec<WideTableRegion>) {
    if text.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let raw_lines: Vec<&str> = text.lines().collect();
    let outcome = parse_markdown_lines(
        &raw_lines,
        MarkdownParseState::default(),
        theme,
        width,
        table_horiz_offset,
    );
    (outcome.lines, outcome.wide_tables)
}

/// Frozen incremental-render state for one streaming markdown message.
///
/// Designed for the single active streaming message: the renderer holds
/// one `Option<MarkdownIncrementalState>` (not per-message) because only
/// the message at `active_stream_index` grows tick-by-tick. On every
/// tick the streaming content is **monotonically growing** (each SSE
/// event delivers the full accumulated text), so the previously-frozen
/// prefix can be reused and only the newly-completed lines are parsed.
///
/// Freeze boundary: the frozen prefix always ends at a complete line
/// (`\n`). The last incomplete line of `content` is never frozen — it is
/// re-parsed every tick (cheap: one line).
///
/// Tables are handled by never freezing a trailing pipe-delimited line
/// (a possible table header/row that could still grow): the freeze point
/// rolls back past any trailing pipe lines, so a streaming table is always
/// re-parsed from its header on each tick. This is correct (column widths
/// depend on all rows) at the cost of re-rendering only the table region.
///
/// This struct deliberately stores only a **line count** (not the `Line`
/// trees themselves): the frozen prefix lines live in the previous tick's
/// `MessageVisualBlock` and are MOVED (zero clone) into the new block by
/// `build_transcript_cache`. The count tells the caller how many lines to
/// move, keeping per-tick work O(suffix) instead of O(total).
#[derive(Clone)]
pub struct MarkdownIncrementalState {
    /// The frozen content prefix (everything up to and including the last
    /// `\n` before the incomplete trailing line, minus trailing pipe
    /// lines). `content` on the next tick must start with this exact
    /// string for the increment to apply.
    frozen_content: String,
    /// Parser state snapshot at the end of `frozen_content`.
    state: MarkdownParseState,
    /// Number of frozen markdown lines carried over from the previous
    /// tick's block. `build_transcript_cache` moves exactly this many
    /// markdown lines from the prev block as the frozen prefix.
    frozen_markdown_line_count: usize,
    /// Width and theme fingerprint; a change invalidates the cache.
    width: u16,
    theme: Theme,
    /// Horizontal window start used for wide tables in the frozen prefix. A
    /// change means the frozen lines were rendered at a different window, so
    /// the incremental cache is invalidated (full re-render this tick).
    table_horiz_offset: usize,
    /// `message.thinking_content.len()` (byte length) when this state was
    /// built. The thinking block sits above the markdown in the frozen
    /// prefix; a length change means the prefix line count is misaligned,
    /// so the caller forces a full fallback.
    ///
    /// **Limitation:** this is a *length-only* fingerprint. It detects
    /// appends (the common streaming case) but NOT same-length rewrites of
    /// `thinking_content` (a correction/replacement at equal byte size). The
    /// current append-only streaming protocol never rewrites thinking in
    /// place, so this is safe today; if the daemon ever does in-place
    /// thinking edits, a stale frozen prefix could be moved and the user
    /// would see the old thinking text until the stream settles. Storing the
    /// message `render_revision` (or a cheap hash) here would close that gap
    /// without keeping the full `thinking_content`.
    thinking_len: usize,
}

impl MarkdownIncrementalState {
    /// The thinking-content length recorded when this state was built.
    pub fn thinking_len(&self) -> usize {
        self.thinking_len
    }

    /// Record the thinking-content length that produced this state. Called
    /// by `render_standard_message_lines` after the markdown render so the
    /// next tick can detect a thinking-block change and fall back to a
    /// full render instead of moving a misaligned frozen prefix.
    pub fn set_thinking_len(&mut self, len: usize) {
        self.thinking_len = len;
    }
}

/// Output of [`render_markdown_incremental`].
pub struct MarkdownIncrementResult {
    /// Markdown lines to append to the block. When
    /// `frozen_markdown_move_count` is `Some` these are the SUFFIX only
    /// (the frozen prefix is moved from the previous tick's block by
    /// `build_transcript_cache`); when `None` (full fallback) these are
    /// the complete markdown output.
    pub lines: Vec<Line<'static>>,
    pub wrapped: Vec<Vec<Line<'static>>>,
    /// Wide tables in this result's `lines`. `start_line` is relative to the
    /// FULL markdown output (frozen prefix + `lines`), so the caller can
    /// rebase them straight onto the message block.
    pub wide_tables: Vec<WideTableRegion>,
    /// `Some(n)` when the incremental path applied: move `n` frozen
    /// markdown lines from the previous block as the prefix. `None` for
    /// the full fallback (use `lines` as the complete output).
    pub frozen_markdown_move_count: Option<usize>,
    pub new_state: MarkdownIncrementalState,
}

/// Render streaming markdown incrementally, reusing the frozen prefix.
///
/// Returns only the **suffix** lines (the newly-parsed remainder) plus the
/// count of frozen markdown lines to move from the previous tick's block.
/// On any cache miss (width/theme change, content is not an extension of
/// the frozen prefix, or no prior state) this degrades to a full
/// [`render_markdown`] pass — always correct, just slower for that tick.
pub fn render_markdown_incremental(
    prev: Option<MarkdownIncrementalState>,
    content: &str,
    theme: &Theme,
    width: u16,
    table_horiz_offset: usize,
) -> MarkdownIncrementResult {
    // Fast path: no content.
    if content.is_empty() {
        return MarkdownIncrementResult {
            lines: Vec::new(),
            wrapped: Vec::new(),
            wide_tables: Vec::new(),
            frozen_markdown_move_count: None,
            new_state: MarkdownIncrementalState {
                frozen_content: String::new(),
                state: MarkdownParseState::default(),
                frozen_markdown_line_count: 0,
                width,
                theme: *theme,
                table_horiz_offset,
                thinking_len: 0,
            },
        };
    }

    // Validate cache. Any mismatch (including a horizontal window change) →
    // full rebuild.
    let can_increment = match &prev {
        Some(p) => {
            p.width == width
                && p.theme == *theme
                && p.table_horiz_offset == table_horiz_offset
                && content.starts_with(&p.frozen_content)
        }
        None => false,
    };

    if !can_increment {
        let (lines, wrapped, state, wide_tables) =
            render_markdown_full(content, theme, width, table_horiz_offset);
        return MarkdownIncrementResult {
            lines,
            wrapped,
            wide_tables,
            frozen_markdown_move_count: None,
            new_state: state,
        };
    }

    let prev = prev.unwrap();
    let remainder = &content[prev.frozen_content.len()..];
    let all_remainder_lines: Vec<&str> = remainder.lines().collect();

    // Freeze boundary: everything up to the last `\n`, minus any trailing
    // pipe-delimited lines (a table header/row may still grow).
    let (complete_part, _trailing) = match remainder.rfind('\n') {
        Some(i) => (&remainder[..=i], &remainder[i + 1..]),
        None => ("", remainder),
    };
    let trimmed_complete = trim_trailing_table_candidate(complete_part);

    // Suffix: parse the whole remainder (complete + trailing lines)
    // together so a table spanning the trailing boundary is rendered as a
    // table, not as separate plain lines. The frozen prefix (already in
    // the previous tick's block) is NOT re-emitted here — eliminating the
    // O(frozen) clone that made the prior implementation O(n²) total.
    let full_outcome = parse_markdown_lines(
        &all_remainder_lines,
        prev.state.clone(),
        theme,
        width,
        table_horiz_offset,
    );
    let suffix_lines = full_outcome.lines;
    let suffix_wrapped: Vec<Vec<Line<'static>>> = suffix_lines
        .iter()
        .map(|line| super::transcript::wrap_line_to_visual_lines(line, width))
        .collect();

    // Frozen outcome: parse only the trimmed complete portion to advance
    // the parser state and count the newly-frozen lines. Because it is a
    // source-prefix of `all_remainder_lines` parsed from the same state,
    // `frozen_outcome.lines.len()` is the output-prefix length of
    // `suffix_lines` that becomes frozen this tick.
    let frozen_outcome = if trimmed_complete.is_empty() {
        MarkdownParseOutcome {
            lines: Vec::new(),
            state: prev.state.clone(),
            wide_tables: Vec::new(),
        }
    } else {
        let trimmed_lines: Vec<&str> = trimmed_complete.lines().collect();
        parse_markdown_lines(
            &trimmed_lines,
            prev.state.clone(),
            theme,
            width,
            table_horiz_offset,
        )
    };

    let new_frozen_count = prev.frozen_markdown_line_count + frozen_outcome.lines.len();
    let mut new_frozen_content = prev.frozen_content;
    new_frozen_content.push_str(trimmed_complete);

    let new_state = MarkdownIncrementalState {
        frozen_content: new_frozen_content,
        state: frozen_outcome.state,
        frozen_markdown_line_count: new_frozen_count,
        width,
        theme: *theme,
        table_horiz_offset,
        thinking_len: prev.thinking_len,
    };

    // Rebase suffix wide-table start lines onto the full markdown output:
    // the suffix's first rendered line sits immediately after the frozen
    // prefix (`prev.frozen_markdown_line_count` logical lines).
    let suffix_wide_tables: Vec<WideTableRegion> = full_outcome
        .wide_tables
        .iter()
        .map(|wt| WideTableRegion {
            start_line: wt.start_line + prev.frozen_markdown_line_count,
            ..*wt
        })
        .collect();

    MarkdownIncrementResult {
        lines: suffix_lines,
        wrapped: suffix_wrapped,
        wide_tables: suffix_wide_tables,
        frozen_markdown_move_count: Some(prev.frozen_markdown_line_count),
        new_state,
    }
}

/// Strip trailing complete pipe-delimited lines from `s` (which must end
/// at a `\n` boundary, or be empty). A trailing pipe line could be the
/// header/row of a table that is still streaming, so it must not be
/// frozen — the next tick re-parses it together with whatever follows.
fn trim_trailing_table_candidate(mut s: &str) -> &str {
    while !s.is_empty() {
        let body = &s[..s.len() - 1]; // drop trailing '\n'
        let last_line_start = match body.rfind('\n') {
            Some(i) => i + 1,
            None => 0,
        };
        let last_line = &body[last_line_start..];
        if split_table_cells(last_line).is_some() {
            s = &s[..last_line_start];
        } else {
            break;
        }
    }
    s
}

/// Full (non-incremental) render, producing a fresh incremental state.
fn render_markdown_full(
    content: &str,
    theme: &Theme,
    width: u16,
    table_horiz_offset: usize,
) -> (
    Vec<Line<'static>>,
    Vec<Vec<Line<'static>>>,
    MarkdownIncrementalState,
    Vec<WideTableRegion>,
) {
    let (logical_lines, wide_tables) =
        render_markdown_with_horiz(content, theme, width, table_horiz_offset);
    let wrapped_lines: Vec<Vec<Line<'static>>> = logical_lines
        .iter()
        .map(|line| super::transcript::wrap_line_to_visual_lines(line, width))
        .collect();

    // Freeze boundary: up to and including the last `\n`, minus trailing
    // pipe lines (a table header/row may still grow).
    let frozen_content = match content.rfind('\n') {
        Some(i) => trim_trailing_table_candidate(&content[..=i]).to_string(),
        None => String::new(),
    };

    // Re-parse the frozen prefix to get the state snapshot and the frozen
    // line count (a separate parse — output line count differs from the
    // source line count because code fences emit zero lines). Only the
    // count is kept; the frozen `Line` trees themselves are not stored —
    // they live in the block and are moved by `build_transcript_cache`.
    let (frozen_count, state) = if frozen_content.is_empty() {
        (0, MarkdownParseState::default())
    } else {
        let frozen_lines: Vec<&str> = frozen_content.lines().collect();
        let outcome = parse_markdown_lines(
            &frozen_lines,
            MarkdownParseState::default(),
            theme,
            width,
            table_horiz_offset,
        );
        (outcome.lines.len(), outcome.state)
    };

    (
        logical_lines,
        wrapped_lines,
        MarkdownIncrementalState {
            frozen_content,
            state,
            frozen_markdown_line_count: frozen_count,
            width,
            theme: *theme,
            table_horiz_offset,
            thinking_len: 0,
        },
        wide_tables,
    )
}

pub fn contains_markdown_table(text: &str) -> bool {
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut in_code_block = false;
    let mut line_index = 0;

    while line_index < raw_lines.len() {
        let trimmed_start = raw_lines[line_index].trim_start();
        if trimmed_start.starts_with("```") {
            in_code_block = !in_code_block;
            line_index += 1;
            continue;
        }

        if !in_code_block && parse_table_block(&raw_lines[line_index..]).is_some() {
            return true;
        }
        line_index += 1;
    }

    false
}

fn parse_table_block(lines: &[&str]) -> Option<(MarkdownTable, usize)> {
    if lines.len() < 2 {
        return None;
    }

    let header = split_table_cells(lines[0])?;
    let aligns = parse_table_separator(lines[1])?;
    if header.len() < 2 || aligns.len() != header.len() {
        return None;
    }

    let mut rows = Vec::new();
    let mut consumed = 2;
    while consumed < lines.len() {
        let Some(cells) = split_table_cells(lines[consumed]) else {
            break;
        };
        if cells.len() != aligns.len() {
            break;
        }
        rows.push(cells);
        consumed += 1;
    }

    Some((
        MarkdownTable {
            header,
            aligns,
            rows,
        },
        consumed,
    ))
}

fn parse_table_separator(line: &str) -> Option<Vec<TableAlign>> {
    let cells = split_table_cells(line)?;
    if cells.len() < 2 {
        return None;
    }

    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let trimmed = cell.trim();
        if trimmed.len() < 3 {
            return None;
        }
        if !trimmed
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        {
            return None;
        }
        let dash_count = trimmed.chars().filter(|ch| *ch == '-').count();
        if dash_count < 3 {
            return None;
        }

        let starts = trimmed.starts_with(':');
        let ends = trimmed.ends_with(':');
        aligns.push(match (starts, ends) {
            (true, true) => TableAlign::Center,
            (false, true) => TableAlign::Right,
            _ => TableAlign::Left,
        });
    }

    Some(aligns)
}

fn split_table_cells(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut start = 0;
    let mut end = chars.len();
    if chars.first() == Some(&'|') {
        start += 1;
    }
    if end > start && chars[end - 1] == '|' && !is_escaped(&chars, end - 1) {
        end -= 1;
    }

    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut i = start;
    while i < end {
        let ch = chars[i];
        if ch == '\\' && i + 1 < end && chars[i + 1] == '|' {
            cell.push('|');
            i += 2;
            continue;
        }
        if ch == '|' && !is_escaped(&chars, i) {
            cells.push(cell.trim().to_string());
            cell.clear();
            i += 1;
            continue;
        }
        cell.push(ch);
        i += 1;
    }
    cells.push(cell.trim().to_string());

    if cells.len() < 2 {
        None
    } else {
        Some(cells)
    }
}

fn is_escaped(chars: &[char], index: usize) -> bool {
    let mut slash_count = 0;
    let mut current = index;
    while current > 0 {
        current -= 1;
        if chars[current] == '\\' {
            slash_count += 1;
        } else {
            break;
        }
    }
    slash_count % 2 == 1
}

/// Render a markdown table. Columns use their natural display width; when the
/// table is wider than `width` it is windowed to
/// `[table_horiz_offset, table_horiz_offset + width)` and a muted hint line
/// is appended. Returns the rendered lines plus, for wide tables,
/// `(natural_width, clamped_offset)` so the caller can record the window.
fn render_table(
    table: &MarkdownTable,
    theme: &Theme,
    width: u16,
    table_horiz_offset: usize,
) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    if table.header.is_empty() {
        return (Vec::new(), None);
    }

    let column_count = table.header.len();
    let column_widths = table_column_widths(&table.header, &table.rows, theme);
    let border_style = Style::default().fg(theme.muted).bg(theme.background);

    let mut rendered = Vec::new();
    rendered.push(render_table_border(
        "┌",
        "┬",
        "┐",
        &column_widths,
        border_style,
    ));
    rendered.push(render_table_row(
        &table.header,
        &table.aligns,
        &column_widths,
        theme,
        true,
        border_style,
    ));
    rendered.push(render_table_border(
        "├",
        "┼",
        "┤",
        &column_widths,
        border_style,
    ));
    for row in &table.rows {
        if row.len() == column_count {
            rendered.push(render_table_row(
                row,
                &table.aligns,
                &column_widths,
                theme,
                false,
                border_style,
            ));
        }
    }
    rendered.push(render_table_border(
        "└",
        "┴",
        "┘",
        &column_widths,
        border_style,
    ));

    // Natural (un-windowed) total display width of the table.
    let natural_width = line_display_width(&rendered[0]);
    let viewport = width as usize;
    if natural_width <= viewport {
        return (rendered, None);
    }

    // Wide table: window every line to `[offset, offset + viewport)`.
    let max_offset = natural_width.saturating_sub(viewport);
    let offset = table_horiz_offset.min(max_offset);
    let mut lines: Vec<Line<'static>> = rendered
        .into_iter()
        .map(|line| slice_line_by_display_columns(&line, offset, offset + viewport))
        .map(|line| pad_line_to_display_columns(line, viewport))
        .collect();
    lines.push(render_table_hint_line(
        theme,
        natural_width,
        offset,
        offset + viewport,
        viewport,
    ));
    (lines, Some((natural_width, offset)))
}

fn table_column_widths(header: &[String], body: &[Vec<String>], theme: &Theme) -> Vec<usize> {
    let column_count = header.len();
    let mut widths = vec![3; column_count];
    for row in std::iter::once(header).chain(body.iter().map(Vec::as_slice)) {
        for (idx, cell) in row.iter().enumerate().take(column_count) {
            widths[idx] = widths[idx].max(inline_display_width(cell, theme));
        }
    }
    widths
}

/// Muted hint line appended after every windowed (wide) table, mirroring the
/// scroll affordances (◀…▶), the visible column range and the total width —
/// e.g. `◀…▶  cols 3-12/18  Alt+←/→`. Downgraded to ASCII where the terminal
/// needs it.
fn render_table_hint_line(
    theme: &Theme,
    natural_width: usize,
    window_start: usize,
    window_end: usize,
    viewport: usize,
) -> Line<'static> {
    let window_end_shown = window_end.min(natural_width);
    // Pick the most informative hint that still fits the viewport. The full
    // form explains both mouse and keyboard affordances; narrower viewports
    // fall back to shorter spellings, and finally to an empty line, so the
    // hint itself never overflows the window it annotates.
    //
    // Candidates are measured *after* `sanitize_terminal_text`, because the
    // ASCII downgrade expands glyphs (`…` -> `...`, `←` -> `<-`); measuring
    // the source text would let a downgraded hint exceed the viewport.
    let variants = [
        format!(
            "  {}  cols {}-{}/{}  {} or Shift+wheel",
            "◀…▶",
            window_start + 1,
            window_end_shown,
            natural_width,
            "Alt+←/→"
        ),
        format!(
            "  {}  cols {}-{}/{}  {}",
            "◀…▶",
            window_start + 1,
            window_end_shown,
            natural_width,
            "Alt+←/→"
        ),
        format!(
            "{} cols {}-{}/{}",
            "◀…▶",
            window_start + 1,
            window_end_shown,
            natural_width
        ),
        format!("{}  …/{}", "◀…▶", natural_width),
    ];
    let text = variants
        .into_iter()
        .map(|candidate| sanitize_terminal_text(&candidate))
        .find(|sanitized| display_width(sanitized) <= viewport)
        .unwrap_or_default();
    Line::styled(
        text,
        Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::ITALIC),
    )
}

/// Slice a styled `Line` to the display-column window `[start, end)`,
/// preserving span styles. The result is never wider than `end - start`
/// columns and, crucially, keeps the source's column grid: a character that
/// straddles the *left* boundary is replaced by as many spaces as its
/// remaining columns, so every following glyph stays in its original column.
///
/// Dropping such a character instead would shift the whole row left by its
/// width while the width-1 border rows stay put, leaving the table's vertical
/// borders misaligned (a double-width CJK/emoji glyph straddling the window
/// start is enough to trigger it). A character straddling the *right* boundary
/// is dropped: the trailing pad added by [`pad_line_to_display_columns`]
/// restores a uniform right edge.
fn slice_line_by_display_columns(line: &Line<'_>, start: usize, end: usize) -> Line<'static> {
    if end <= start {
        return Line::from(String::new());
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut column = 0usize;
    for span in &line.spans {
        let mut segment = String::new();
        for ch in span.content.chars() {
            let width = UnicodeWidthChar::width(ch).unwrap_or(0);
            let next_column = column + width;
            if next_column <= start {
                // Entirely left of the window.
            } else if column >= start && next_column <= end {
                segment.push(ch);
            } else if column < start {
                // Straddles the left boundary: keep its visible columns as
                // blanks so the remaining glyphs do not slide left.
                segment.push_str(&" ".repeat(next_column - start));
            }
            // Straddling the right boundary: dropped, padded later.
            column = next_column;
        }
        if !segment.is_empty() {
            spans.push(Span::styled(segment, span.style));
        }
    }
    Line::from(spans)
}

/// Right-pad a `Line` to exactly `target` display columns with plain spaces.
///
/// Wide-table windowing feeds every line through
/// [`slice_line_by_display_columns`], which drops characters that straddle the
/// window boundary. With full-width (CJK) cell content the sliced line can end
/// one display column short of the viewport (the trailing pad/border is
/// dropped while the border lines, made of width-1 runes, keep a full row),
/// so the right edge becomes jagged and the table border no longer reads as a
/// single vertical line. Padding to the full window width restores a uniform
/// cut at the right edge, matching how ASCII-only wide tables already look.
fn pad_line_to_display_columns(line: Line<'static>, target: usize) -> Line<'static> {
    let width = line_display_width(&line);
    let padding = target.saturating_sub(width);
    if padding == 0 {
        return line;
    }
    let mut spans = line.spans;
    spans.push(Span::raw(" ".repeat(padding)));
    Line::from(spans)
}

fn render_table_border(
    left: &str,
    separator: &str,
    right: &str,
    widths: &[usize],
    style: Style,
) -> Line<'static> {
    let mut text = String::new();
    text.push_str(left);
    for (idx, width) in widths.iter().enumerate() {
        if idx > 0 {
            text.push_str(separator);
        }
        text.push_str(&"─".repeat(width + 2));
    }
    text.push_str(right);

    Line::from(vec![Span::styled(sanitize_terminal_text(&text), style)])
}

fn render_table_row(
    cells: &[String],
    aligns: &[TableAlign],
    widths: &[usize],
    theme: &Theme,
    is_header: bool,
    border_style: Style,
) -> Line<'static> {
    let mut spans = vec![Span::styled(sanitize_terminal_text("│"), border_style)];
    for (idx, width) in widths.iter().enumerate() {
        let cell = cells.get(idx).map(String::as_str).unwrap_or_default();
        let mut cell_line = truncate_inline_line(parse_inline(cell, theme), *width);
        let cell_width = line_display_width(&cell_line);
        let extra = width.saturating_sub(cell_width);
        let (left_pad, right_pad) = match aligns.get(idx).copied().unwrap_or(TableAlign::Left) {
            TableAlign::Left => (0, extra),
            TableAlign::Right => (extra, 0),
            TableAlign::Center => (extra / 2, extra - (extra / 2)),
        };

        spans.push(Span::raw(" ".repeat(left_pad + 1)));
        if is_header {
            for span in &mut cell_line.spans {
                span.style = span
                    .style
                    .fg(theme.accent)
                    .bg(theme.background)
                    .add_modifier(Modifier::BOLD);
            }
        }
        spans.extend(cell_line.spans);
        spans.push(Span::raw(" ".repeat(right_pad + 1)));
        spans.push(Span::styled(sanitize_terminal_text("│"), border_style));
    }

    Line::from(spans)
}

fn inline_display_width(cell: &str, theme: &Theme) -> usize {
    line_display_width(&parse_inline(cell, theme))
}

fn line_display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

fn truncate_inline_line(line: Line<'static>, width: usize) -> Line<'static> {
    if line_display_width(&line) <= width {
        return line;
    }

    if width <= 1 {
        return Line::from(vec![Span::styled("…", Style::default())]);
    }

    let mut spans = Vec::new();
    let mut used = 0;
    let ellipsis_width = display_width("…");

    'outer: for span in line.spans {
        let mut segment = String::new();
        for ch in span.content.chars() {
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + char_width + ellipsis_width > width {
                if !segment.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut segment), span.style));
                }
                spans.push(Span::styled("…", span.style));
                break 'outer;
            }
            segment.push(ch);
            used += char_width;
        }
        if !segment.is_empty() {
            spans.push(Span::styled(segment, span.style));
        }
    }

    Line::from(spans)
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

fn parse_inline(line: &str, theme: &Theme) -> Line<'static> {
    if line.is_empty() {
        return Line::from(String::new());
    }

    let chars: Vec<char> = line.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buffer = String::new();

    let mut in_bold = false;
    let mut in_italic = false;
    let mut in_inline_code = false;

    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];

        if ch == '`' {
            if in_inline_code {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_inline_code = false;
                i += 1;
                continue;
            }

            if chars[i + 1..].contains(&'`') {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_inline_code = true;
                i += 1;
                continue;
            }

            buffer.push(ch);
            i += 1;
            continue;
        }

        if !in_inline_code && ch == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if in_bold {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_bold = false;
                i += 2;
                continue;
            }

            if has_closing_double_asterisk(&chars, i + 2) {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_bold = true;
                i += 2;
                continue;
            }

            buffer.push('*');
            buffer.push('*');
            i += 2;
            continue;
        }

        if !in_inline_code && ch == '*' {
            if in_italic {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_italic = false;
                i += 1;
                continue;
            }

            if has_closing_single_asterisk(&chars, i + 1) {
                push_buffer(
                    &mut spans,
                    &mut buffer,
                    current_inline_style(theme, in_bold, in_italic, in_inline_code),
                );
                in_italic = true;
                i += 1;
                continue;
            }

            buffer.push('*');
            i += 1;
            continue;
        }

        buffer.push(ch);
        i += 1;
    }

    push_buffer(
        &mut spans,
        &mut buffer,
        current_inline_style(theme, in_bold, in_italic, in_inline_code),
    );

    if spans.is_empty() {
        return Line::from(String::new());
    }

    Line::from(spans)
}

fn parse_numbered_prefix(line: &str) -> Option<(&str, &str)> {
    let mut split_idx = 0;
    for (idx, ch) in line.char_indices() {
        if ch.is_ascii_digit() {
            split_idx = idx + ch.len_utf8();
            continue;
        }
        break;
    }

    if split_idx == 0 {
        return None;
    }

    let rest = &line[split_idx..];
    if !rest.starts_with(". ") {
        return None;
    }

    let prefix = &line[..split_idx + 1];
    let content = &rest[2..];
    Some((prefix, content))
}

fn has_closing_double_asterisk(chars: &[char], start: usize) -> bool {
    let mut i = start;
    while i + 1 < chars.len() {
        if chars[i] == '*' && chars[i + 1] == '*' {
            return true;
        }
        i += 1;
    }
    false
}

fn has_closing_single_asterisk(chars: &[char], start: usize) -> bool {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '*' {
            let prev_is_star = i > 0 && chars[i - 1] == '*';
            let next_is_star = i + 1 < chars.len() && chars[i + 1] == '*';
            if !prev_is_star && !next_is_star {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn current_inline_style(
    theme: &Theme,
    in_bold: bool,
    in_italic: bool,
    in_inline_code: bool,
) -> Style {
    let mut style = if in_inline_code {
        Style::default().fg(theme.code_fg).bg(theme.code_bg)
    } else {
        Style::default().fg(theme.foreground).bg(theme.background)
    };

    if !in_inline_code {
        if in_bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if in_italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
    }

    style
}

fn push_buffer(spans: &mut Vec<Span<'static>>, buffer: &mut String, style: Style) {
    if buffer.is_empty() {
        return;
    }

    spans.push(Span::styled(
        sanitize_terminal_text(&std::mem::take(buffer)),
        style,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        Theme::detect()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn renders_markdown_table_as_terminal_table() {
        let lines = render_markdown(
            "| Name | Status |\n| --- | --- |\n| xiaoO | ready |",
            &test_theme(),
            80,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text.len(), 5);
        assert!(text[0].starts_with('┌'));
        assert!(text[1].contains("Name"));
        assert!(text[1].contains("Status"));
        assert!(text[2].starts_with('├'));
        assert!(text[3].contains("xiaoO"));
        assert!(text[3].contains("ready"));
        assert!(text[4].starts_with('└'));
    }

    #[test]
    fn table_separator_requires_three_dashes() {
        let lines = render_markdown("| A | B |\n| - | - |\n| 1 | 2 |", &test_theme(), 80);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text[0], "| A | B |");
        assert_eq!(text[1], "| - | - |");
        assert_eq!(text[2], "| 1 | 2 |");
    }

    #[test]
    fn table_rendering_truncates_to_available_width() {
        // A table wider than the viewport is no longer squeezed or ellipsised:
        // columns keep their natural width, every rendered line is windowed to
        // the viewport, the wide-table metadata is reported and a muted hint
        // row is appended so the table can be panned horizontally.
        let (lines, wide) = render_markdown_with_horiz(
            "| Column A | Column B |\n| --- | --- |\n| a very long value | another long value |",
            &test_theme(),
            24,
            0,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(text.iter().all(|line| display_width(line) <= 24));
        // Cell content is never ellipsised by the window; the trailing hint
        // row's `…` is a decorative scroll affordance (◀…▶), not truncation.
        assert!(text
            .iter()
            .take(text.len() - 1)
            .all(|line| !line.contains('…')));
        assert_eq!(wide.len(), 1);
        assert!(wide[0].natural_width > 24);
        assert_eq!(wide[0].horiz_offset, 0);
        assert_eq!(wide[0].viewport_width, 24);
        // The trailing hint row mirrors the scroll affordance + window.
        assert!(text.last().unwrap().contains("cols"));
        assert!(text.last().unwrap().contains('/'));
    }

    #[test]
    fn wide_table_renders_in_full_when_viewport_is_sufficient() {
        // A viewport wide enough to hold the natural table shows the full
        // content with no omission and no wide-table metadata.
        let long_cell = "c".repeat(120);
        let content = format!("| A | B |\n| --- | --- |\n| {long_cell} | tail |");
        let (lines, wide) = render_markdown_with_horiz(&content, &test_theme(), 160, 0);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        assert!(wide.is_empty());
        assert!(text.iter().all(|line| display_width(line) <= 160));
        assert!(text.join("\n").contains(&long_cell));
        assert!(!text.join("\n").contains('…'));
    }

    #[test]
    fn different_horiz_offsets_produce_different_windows() {
        let content =
            "| Left | Middle | Right |\n| --- | --- | --- |\n| 11111 | 2222222222222222 | 33333 |";
        let (l0, _w0) = render_markdown_with_horiz(content, &test_theme(), 20, 0);
        let (l8, _w8) = render_markdown_with_horiz(content, &test_theme(), 20, 8);
        let (lmax, wmax) = render_markdown_with_horiz(content, &test_theme(), 20, 1000);
        let t0 = l0.iter().map(line_text).collect::<Vec<_>>();
        let t8 = l8.iter().map(line_text).collect::<Vec<_>>();
        let tmax = lmax.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(wmax.len(), 1);
        assert_eq!(t0.len(), t8.len());
        // Moving the window by 8 columns changes the visible slice.
        assert_ne!(t0[1], t8[1]);
        assert_ne!(t0[3], t8[3]);
        // Every windowed line (including the hint row) stays in the viewport.
        assert!(t0.iter().all(|l| display_width(l) <= 20));
        assert!(t8.iter().all(|l| display_width(l) <= 20));
        assert!(tmax.iter().all(|l| display_width(l) <= 20));
        // The window start shifts the visible content: the leftmost window
        // shows the left cell, the offset-8 window shows the middle cell,
        // and the huge (clamped-to-max) offset reveals the right-hand cell.
        assert!(t0[1].contains("Left"));
        assert!(t8[1].contains("Middle"));
        assert!(tmax[3].contains("33333"));
    }

    #[test]
    fn horiz_window_slices_by_display_columns_and_keeps_styles() {
        use ratatui::style::Color;
        let bold = Style::default().fg(Color::Red);
        // 你 (w2) 好 (w2) y (w1) o (w1) 📁 (w2) u (w1) x (w1)
        let line = Line::from(vec![
            Span::styled("你", bold),
            Span::styled("好yo", Style::default()),
            Span::styled("📁ux", bold),
        ]);
        let sliced = slice_line_by_display_columns(&line, 2, 8);
        let text = line_text(&sliced);
        // Window [2,8): keeps 好(2-4) y(4-5) o(5-6) 📁(6-8); 你 and u/x fall
        // outside/straddle the boundary and are dropped.
        assert_eq!(text, "好yo📁");
        assert!(display_width(&text) <= 6);
        assert_eq!(sliced.spans[0].content, "好yo");
        assert!(sliced.spans[0].style.fg.is_none());
        assert_eq!(sliced.spans[1].content, "📁");
        assert_eq!(sliced.spans[1].style.fg, Some(Color::Red));
    }

    /// Display column each border/junction glyph of a rendered table row sits
    /// at. A correctly windowed table has the same set for every row (that is
    /// what makes the vertical borders read as straight lines).
    fn border_glyph_columns(text: &str) -> Vec<usize> {
        const GLYPHS: &str =
            "\u{2502}\u{250c}\u{2510}\u{2514}\u{2518}\u{251c}\u{2524}\u{252c}\u{2534}\u{253c}";
        let mut columns = Vec::new();
        let mut column = 0usize;
        for ch in text.chars() {
            if GLYPHS.contains(ch) {
                columns.push(column);
            }
            column += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        columns
    }

    #[test]
    fn wide_cjk_table_window_keeps_uniform_right_edge() {
        // Regression: windowing a wide CJK table used to end the data row one
        // display column short of the viewport. A full-width character filled
        // the last columns so the trailing pad and the cell border (both
        // width-1) were dropped, while the border lines (made of width-1
        // runes) stayed full width — leaving a jagged right edge. Every
        // windowed line must fill the viewport exactly so the cut is uniform.
        let content = "| 名字 | 描述 |\n| --- | --- |\n| 小明 | 一个比较长的中文字符串用来测试 |";
        let viewport = 40u16;
        let (lines, wide) = render_markdown_with_horiz(content, &test_theme(), viewport, 0);
        assert_eq!(wide.len(), 1);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        // All table lines (excluding the trailing hint row) fill the viewport.
        for (idx, t) in text.iter().enumerate().take(text.len() - 1) {
            assert_eq!(
                display_width(t),
                viewport as usize,
                "table line {idx} must fill the viewport"
            );
        }
        // The data row (previously 1 column short) must be exactly viewport wide.
        assert_eq!(display_width(&text[3]), viewport as usize);
    }

    #[test]
    fn slice_keeps_the_column_grid_when_a_wide_glyph_straddles_the_left_edge() {
        // 你 (cols 0-2) 好 (2-4) y (4-5) o (5-6).
        let line = Line::from("你好yo");
        // Window [1,6): 你 straddles the left edge, so its one visible column
        // becomes a blank instead of being dropped — 好 must stay in column 1.
        assert_eq!(
            line_text(&slice_line_by_display_columns(&line, 1, 6)),
            " 好yo"
        );
        // Dropping it (the pre-fix behaviour) would have produced "好yo",
        // sliding 好 to column 0 and misaligning the row against the width-1
        // border rows.
        assert_eq!(
            line_text(&slice_line_by_display_columns(&line, 2, 6)),
            "好yo"
        );
        // A wide glyph straddling the *right* edge is still dropped, and the
        // pad restores the uniform right edge.
        let clipped = slice_line_by_display_columns(&line, 0, 3);
        assert_eq!(line_text(&clipped), "你");
        let padded = pad_line_to_display_columns(clipped, 3);
        assert_eq!(display_width(&line_text(&padded)), 3);
    }

    #[test]
    fn wide_table_windows_keep_the_column_grid_at_every_reachable_offset() {
        // Regression: a double-width (CJK/emoji) glyph straddling the window
        // start was dropped instead of replaced by blanks, shifting every
        // later glyph of that row one column left while the width-1 border
        // rows stayed put. Padding the right edge only equalized line
        // *lengths*, so the borders stayed jagged (e.g. viewport 40 with the
        // first -> step of 13, or viewport 25 at the rightmost window).
        //
        // Invariant checked here: at every offset the arrows/wheel can reach,
        // every row of the windowed table puts its border glyphs in the same
        // display columns, and every row is exactly viewport wide.
        let long: String = "中文内容测试".repeat(10);
        let tables = [
            format!("| 名字 | 描述 |\n| --- | --- |\n| 小明 | {long} |"),
            format!("| A | B |\n| --- | --- |\n| {long} | tail |"),
            format!("| 📁 | 说明 |\n| --- | --- |\n| x | {long} |"),
        ];
        let theme = test_theme();
        for content in &tables {
            for viewport in [20u16, 25, 30, 40, 60, 80, 120] {
                let (_, wide) = render_markdown_with_horiz(content, &theme, viewport, 0);
                assert_eq!(wide.len(), 1, "viewport {viewport} should window");
                let max_offset = wide[0].natural_width - viewport as usize;
                let step = std::cmp::max(8usize, viewport as usize / 3);

                // Exactly the offsets the app reaches: 0, step, 2*step, ... and
                // the clamped rightmost window.
                let mut offsets = vec![0usize];
                let mut offset = step;
                while offset < max_offset {
                    offsets.push(offset);
                    offset += step;
                }
                offsets.push(max_offset);

                for offset in offsets {
                    let (lines, _) = render_markdown_with_horiz(content, &theme, viewport, offset);
                    let text: Vec<String> = lines.iter().map(|l| line_text(l)).collect();
                    let rows = &text[..text.len() - 1]; // drop the hint row
                    let expected = border_glyph_columns(&rows[0]);
                    for (idx, row) in rows.iter().enumerate() {
                        assert_eq!(
                            display_width(row),
                            viewport as usize,
                            "viewport {viewport} offset {offset}: row {idx} width"
                        );
                        assert_eq!(
                            border_glyph_columns(row),
                            expected,
                            "viewport {viewport} offset {offset}: row {idx} border \
                             columns drifted from the top border row\nrow0: {:?}\nrow{idx}: {row:?}",
                            rows[0]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn pad_line_to_display_columns_pads_up_to_target_and_noops_when_larger() {
        // Padding is monotone: it only ever adds columns up to `target`; a
        // target no larger than the current width must leave the line
        // untouched (saturating, never truncates).
        let line = Line::from(vec![Span::raw("你好o")]); // display width 5
        assert_eq!(
            line_text(&pad_line_to_display_columns(line.clone(), 8)),
            "你好o   "
        );
        assert_eq!(
            line_text(&pad_line_to_display_columns(line.clone(), 5)),
            "你好o"
        );
        assert_eq!(
            line_text(&pad_line_to_display_columns(line.clone(), 3)),
            "你好o"
        );
        assert_eq!(
            line_text(&pad_line_to_display_columns(line.clone(), 0)),
            "你好o"
        );
    }

    #[test]
    fn table_exactly_viewport_width_is_not_windowed() {
        // Exact-fit boundary: natural_width == viewport must keep the full
        // table with its real corners (┐/┘), no windowing and no hint row.
        let content = "| A | B |\n| --- | --- |\n| 1 | 2 |";
        let viewport = 13u16; // matches the natural width of this table
        let (lines, wide) = render_markdown_with_horiz(content, &test_theme(), viewport, 0);
        assert!(wide.is_empty());
        let text = lines.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(text.len(), 5);
        assert!(text[0].contains('┐'));
        assert!(text[4].contains('┘'));
        for t in &text {
            assert!(display_width(t) <= viewport as usize);
        }
    }

    #[test]
    fn wide_table_narrow_viewport_never_overflows_or_panics() {
        // Degenerate narrow viewports must neither panic nor overflow: every
        // table line stays exactly viewport-wide (uniform right edge), and
        // the wide-table metadata is reported so panning stays possible.
        let content = "| 名称 | 状态 |\n| --- | --- |\n| 小明 | 一个比较长的中文字符串用来测试 |";
        for viewport in [1u16, 2, 3, 4, 6, 8] {
            let (lines, wide) = render_markdown_with_horiz(content, &test_theme(), viewport, 0);
            assert_eq!(
                wide.len(),
                1,
                "viewport {viewport} should trigger windowing"
            );
            let text = lines.iter().map(line_text).collect::<Vec<_>>();
            assert!(text.len() >= 2);
            // All table lines (excluding the trailing hint row) fit exactly.
            for (idx, t) in text.iter().enumerate().take(text.len() - 1) {
                assert_eq!(
                    display_width(t),
                    viewport as usize,
                    "viewport {viewport} table line {idx}"
                );
            }
        }
    }

    #[test]
    fn wide_table_hint_never_overflows_its_viewport() {
        // The hint is measured *after* the ASCII downgrade and falls back to an
        // empty line, so it can never be wider than the window it annotates —
        // including the degenerate widths where no spelling fits.
        let long: String = "中文内容测试".repeat(10); // 60 CJK chars -> natural width 131
        let content = format!("| 名称 | 状态 |\n| --- | --- |\n| 小明 | {long} |");
        for viewport in [1u16, 2, 3, 4, 5, 8, 12, 20, 24, 30, 40, 60, 80] {
            let (lines, wide) = render_markdown_with_horiz(&content, &test_theme(), viewport, 0);
            assert_eq!(wide.len(), 1, "viewport {viewport} should window");
            let hint = line_text(lines.last().expect("hint row"));
            assert!(
                display_width(&sanitize_terminal_text(&hint)) <= viewport as usize,
                "viewport {viewport}: hint {hint:?} overflows"
            );
        }
    }

    #[test]
    fn hint_line_only_appears_for_wide_tables() {
        let narrow = "| Name | Status |\n| --- | --- |\n| xiaoO | ready |";
        let (lines_narrow, wide_narrow) = render_markdown_with_horiz(narrow, &test_theme(), 40, 0);
        assert!(wide_narrow.is_empty());
        assert!(!line_text(lines_narrow.last().unwrap()).contains("cols"));

        let wide =
            "| Name | A very long header column |\n| --- | --- |\n| xiaoO | another long value |";
        let (lines_wide, wide_meta) = render_markdown_with_horiz(wide, &test_theme(), 20, 0);
        assert_eq!(wide_meta.len(), 1);
        assert_eq!(lines_wide.len(), 6); // 5 table lines + 1 hint
        let hint = line_text(lines_wide.last().unwrap());
        assert!(hint.contains("cols"));
        assert!(hint.contains('/'));
        assert!(!hint.contains("列"), "hint must be English: {hint:?}");
        // Compare against the *downgraded* arrows: on Windows/WSL terminals the
        // hint renders as `<...>` instead of `◀…▶`.
        assert!(hint.contains(&sanitize_terminal_text("◀")));
        assert!(hint.contains(&sanitize_terminal_text("▶")));
    }

    #[test]
    fn incremental_wide_table_metadata_matches_full() {
        let content = "| A | B |\n| --- | --- |\n| some-long-value-here | tail |\nDone.";
        let theme = test_theme();
        let (_, full_wide) = render_markdown_with_horiz(content, &theme, 20, 0);

        let mut state: Option<MarkdownIncrementalState> = None;
        let mut accumulated: Vec<Line<'static>> = Vec::new();
        let mut wide_accum: Vec<WideTableRegion> = Vec::new();
        for (_start, end) in chunk_spans(content.len(), 7) {
            let result = render_markdown_incremental(state, &content[..end], &theme, 20, 0);
            state = Some(result.new_state);
            match result.frozen_markdown_move_count {
                None => {
                    accumulated = result.lines;
                    wide_accum = result.wide_tables;
                }
                Some(frozen_n) => {
                    // Mirrors build_transcript_cache: keep tables fully below
                    // the frozen boundary, append the suffix's new tables.
                    accumulated.truncate(frozen_n);
                    accumulated.extend(result.lines);
                    wide_accum.retain(|wt| wt.start_line + wt.line_count <= frozen_n);
                    wide_accum.extend(result.wide_tables);
                }
            }

            let expected_lines = render_markdown(&content[..end], &theme, 20);
            let expected_wide = render_markdown_with_horiz(&content[..end], &theme, 20, 0).1;
            assert_eq!(
                accumulated.iter().map(line_text).collect::<Vec<_>>(),
                expected_lines.iter().map(line_text).collect::<Vec<_>>(),
                "lines mismatch at prefix {:?}",
                &content[..end]
            );
            assert_eq!(
                wide_accum,
                expected_wide,
                "wide-table metadata mismatch at prefix {:?}",
                &content[..end]
            );
        }
        assert_eq!(wide_accum, full_wide);
    }

    #[test]
    fn table_alignment_uses_rendered_inline_width() {
        let lines = render_markdown(
            "| 类型 | 名称 | 大小 |\n| --- | --- | ---: |\n| 📁 | `.cargo/` | - |\n| 📄 | `README.md` | 9.1 KB |",
            &test_theme(),
            80,
        );
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        let header_columns = vertical_border_columns(&text[1]);
        let first_row_columns = vertical_border_columns(&text[3]);
        let second_row_columns = vertical_border_columns(&text[4]);

        assert_eq!(first_row_columns, header_columns);
        assert_eq!(second_row_columns, header_columns);
    }

    #[test]
    fn detects_markdown_tables_outside_code_blocks() {
        assert!(contains_markdown_table(
            "before\n| A | B |\n| --- | --- |\n| 1 | 2 |"
        ));
        assert!(!contains_markdown_table(
            "```\n| A | B |\n| --- | --- |\n| 1 | 2 |\n```"
        ));
    }

    fn vertical_border_columns(line: &str) -> Vec<usize> {
        let mut columns = Vec::new();
        let mut width = 0;
        for ch in line.chars() {
            if ch == '│' {
                columns.push(width);
            }
            width += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        columns
    }

    /// Render `content` incrementally, feeding it in growing chunks (as a
    /// stream would), and assert the final output matches a one-shot
    /// [`render_markdown`] plus that each intermediate output is identical
    /// to the full render of the content seen so far.
    fn assert_incremental_equals_full(content: &str) {
        let theme = test_theme();
        let width = 40;
        let full = render_markdown(content, &theme, width);
        let full_text = full.iter().map(line_text).collect::<Vec<_>>();

        // Feed chunk-by-chunk at every possible split point to exercise the
        // incremental path thoroughly. The incremental path returns a
        // SUFFIX only (the frozen prefix is moved from the previous tick's
        // block by build_transcript_cache), so we reconstruct the full
        // output by truncating to the frozen count then extending.
        for split in 1..=content.len() {
            let mut state: Option<MarkdownIncrementalState> = None;
            let mut accumulated: Vec<Line<'static>> = Vec::new();
            for (_start, end) in chunk_spans(content.len(), split) {
                let result = render_markdown_incremental(state, &content[..end], &theme, width, 0);
                state = Some(result.new_state);
                match result.frozen_markdown_move_count {
                    None => accumulated = result.lines,
                    Some(frozen_n) => {
                        accumulated.truncate(frozen_n);
                        accumulated.extend(result.lines);
                    }
                }

                let expected = render_markdown(&content[..end], &theme, width);
                let expected_text = expected.iter().map(line_text).collect::<Vec<_>>();
                let actual_text = accumulated.iter().map(line_text).collect::<Vec<_>>();
                assert_eq!(
                    actual_text,
                    expected_text,
                    "incremental mismatch at prefix {:?}",
                    &content[..end]
                );
            }
            assert_eq!(
                accumulated.iter().map(line_text).collect::<Vec<_>>(),
                full_text,
                "final incremental output differs from full render (split={split})"
            );
        }
    }

    /// Generate a sequence of `(start, end)` byte spans covering `len`
    /// bytes in `chunk_size`-byte steps (the last chunk absorbs the rest).
    fn chunk_spans(len: usize, chunk_size: usize) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut start = 0;
        while start < len {
            let end = (start + chunk_size).min(len);
            spans.push((start, end));
            start = end;
        }
        spans
    }

    #[test]
    fn incremental_matches_full_for_prose() {
        assert_incremental_equals_full(
            "Hello world, this is a streaming message.\nSecond line here.\nThird line.",
        );
    }

    #[test]
    fn incremental_matches_full_for_headings_and_lists() {
        assert_incremental_equals_full(
            "# Title\n## Section\n### Subsection\n- item one\n- item two\n1. first\n2. second\nplain text line",
        );
    }

    #[test]
    fn incremental_matches_full_for_code_block() {
        assert_incremental_equals_full(
            "Before the block.\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\nAfter the block.",
        );
    }

    #[test]
    fn incremental_matches_full_for_hr() {
        assert_incremental_equals_full("top\n---\nbottom\n***\nend");
    }

    #[test]
    fn incremental_matches_full_when_table_present() {
        assert_incremental_equals_full(
            "| Name | Status |\n| --- | --- |\n| xiaoO | ready |\n\nAfter the table.",
        );
    }

    #[test]
    fn incremental_handles_empty_and_single_line() {
        let theme = test_theme();
        let result = render_markdown_incremental(None, "", &theme, 40, 0);
        assert!(result.lines.is_empty());

        // "single line" starts with the empty frozen prefix, so the
        // incremental path applies with 0 frozen lines → suffix == full.
        let result2 =
            render_markdown_incremental(Some(result.new_state), "single line", &theme, 40, 0);
        let full = render_markdown("single line", &theme, 40);
        assert_eq!(
            result2.lines.iter().map(line_text).collect::<Vec<_>>(),
            full.iter().map(line_text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn incremental_full_render_is_reference_identical() {
        // A full render through the incremental path (no prior state) must
        // be byte-identical in text to `render_markdown`.
        let content = "# H\n\n```\ncode\n```\n\n- a\n- b\n\ntail";
        let theme = test_theme();
        let width = 30;
        let result = render_markdown_incremental(None, content, &theme, width, 0);
        let full = render_markdown(content, &theme, width);
        assert_eq!(
            result.lines.iter().map(line_text).collect::<Vec<_>>(),
            full.iter().map(line_text).collect::<Vec<_>>()
        );
    }

    /// Benchmark: stream a large prose document tick-by-tick (one word per
    /// tick), comparing full re-render (`render_markdown`) vs the
    /// incremental path (`render_markdown_incremental`). Prints a ratio.
    #[test]
    fn incremental_streaming_benchmark() {
        let theme = test_theme();
        let width = 80;

        // ~180 lines of multi-line prose (newline-separated so the freeze
        // boundary can advance — mirrors real streaming).
        let content = (0..180)
            .map(|i| {
                if i % 5 == 0 {
                    "paragraph-line-with-more-words"
                } else if i % 3 == 0 {
                    "- list item"
                } else {
                    "word"
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let total_words = content.split_whitespace().count();
        let _ = total_words;

        // Incremental: grow content one line per tick (push_str, O(1) per
        // tick — mirrors the real streaming model where the message content
        // is the full accumulated text).
        let mut state: Option<MarkdownIncrementalState> = None;
        let mut incremental_elapsed = std::time::Duration::ZERO;
        let mut acc = String::new();
        let mut acc_lines = 0usize;
        for line in content.lines() {
            if acc_lines > 0 {
                acc.push('\n');
            }
            acc.push_str(line);
            acc_lines += 1;
            let t0 = std::time::Instant::now();
            let result = render_markdown_incremental(state, &acc, &theme, width, 0);
            state = Some(result.new_state);
            incremental_elapsed += t0.elapsed();
        }

        // Full: re-render the whole accumulated content once per tick.
        let mut full_elapsed = std::time::Duration::ZERO;
        let mut acc2 = String::new();
        let mut acc2_lines = 0usize;
        for line in content.lines() {
            if acc2_lines > 0 {
                acc2.push('\n');
            }
            acc2.push_str(line);
            acc2_lines += 1;
            let t0 = std::time::Instant::now();
            let _lines = render_markdown(&acc2, &theme, width);
            full_elapsed += t0.elapsed();
        }

        let ratio = full_elapsed.as_secs_f64() / incremental_elapsed.as_secs_f64().max(1e-9);
        eprintln!(
            "PERF markdown streaming: {} lines/turns — full={:?} incremental={:?} ({:.1}×)",
            acc_lines, full_elapsed, incremental_elapsed, ratio
        );
    }
}
