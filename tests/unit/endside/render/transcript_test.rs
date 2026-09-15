use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::chat::{Message, ToolExecutionStatus, ToolExecutionUpdate};
use crate::theme::Theme;

use super::{
    apply_expanded_tool_panel, build_side_by_side_diff_rows, build_transcript_cache,
    diff_change_counts, expanded_tool_background, highlight_line_selection, parse_file_edit_args,
    parse_join_subagent_terminal, parse_spawn_subagent_agent_id, render_file_edit_tool_lines,
    render_message_entry, render_tool_message_lines, wrap_line_to_visual_lines, WideTableRegion,
};
use crate::app_state::CachedMessageRender;
use crate::app_state::{MessageVisualBlock, TranscriptRenderCache};

fn line_display_width(line: &Line<'static>) -> usize {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

/// Regression test for the "missing last line" bug. `build_transcript_cache`
/// must derive `total_lines` from the same `wrap_line_to_visual_lines` call
/// that produces `visual_lines`, so the two stay in lock-step and the
/// stick-to-bottom viewport never slices off the trailing visual line(s).
/// Long paths/URLs force textwrap to emit more visual lines than a naive
/// character-based `div_ceil` predictor would, which previously left the
/// trailing line stuck below the viewport.
#[test]
fn build_transcript_cache_keeps_total_lines_in_sync_with_visual_lines() {
    // Wrap width that triggers the predictor/wrap mismatch: the long
    // synthetic path (45 chars in parens, > width) forces textwrap to emit
    // 3 visual lines while a character-based predictor only sees 2.
    let wrap_width: u16 = 40;
    let mismatch_text =
        "Session snapshot saved: name (/tmp/xiaoo-test/sessions/snapshot-name.json)";
    let mismatch_line = Line::from(mismatch_text);
    let lines = vec![Line::from("System header"), mismatch_line, Line::raw("")];
    let wrapped_lines: Vec<Vec<Line<'static>>> = lines
        .iter()
        .map(|line| wrap_line_to_visual_lines(line, wrap_width))
        .collect();
    let render = CachedMessageRender {
        width: wrap_width,
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
        wrapped_lines: Some(wrapped_lines),
        lines,
        frozen_prefix_line_count: None,
    };

    let cache = build_transcript_cache(None, vec![Some(render)]);

    // `total_lines` must equal the actual number of visual lines across all
    // blocks, so stick_to_bottom's `scroll_offset = total_lines -
    // inner_height` never points past the last real visual line.
    let actual_visual_lines: usize = cache
        .message_blocks
        .iter()
        .map(|b| b.visual_lines.len())
        .sum();
    assert_eq!(
        cache.total_lines, actual_visual_lines,
        "total_lines must match the sum of block visual_lines.len() so \
             stick_to_bottom scroll_offset (= total_lines - inner_height) \
             never points past the last actual visual line"
    );
    let flat = cache.collect_visible_visual_lines(0, cache.total_lines);
    // The last non-empty visual line (before the trailing empty spacer)
    // must be the path tail that textwrap split off — the exact line that
    // used to be hidden below the viewport.
    let last_content_visual: String = flat
        .iter()
        .rev()
        .skip(1) // skip the trailing empty spacer
        .next()
        .expect("cache should have at least one content line")
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    // The tail must be a substring of the original text, proving it
    // wasn't dropped.
    assert!(
        mismatch_text.contains(&last_content_visual),
        "last content visual line {last_content_visual:?} must be a substring of the original text"
    );
}

/// Regression test for the `rebuild_lines_with_styles` style-offset bug.
///
/// textwrap strips inter-word whitespace at wrap boundaries, so each
/// visual line's text is a contiguous substring of the original but not
/// necessarily at a contiguous running offset. Before the fix,
/// `global_char_offset` was advanced by `line_char_count` only, drifting
/// past the stripped spaces and looking up the wrong style for chars at
/// span boundaries. After the fix, `find_substring_from` realigns
/// `global_char_offset` to each visual line's actual position in the
/// original text.
#[test]
fn rebuild_lines_with_styles_preserves_style_positions_after_whitespace_stripping() {
    // Three styled spans separated by single spaces: "AAA BBB CCC".
    // At width 4, textwrap emits ["AAA", "BBB", "CCC"] (spaces stripped).
    // Without the fix, BBB's chars would get the style at offsets 3/4/5
    // (space + first two italic chars) instead of 4/5/6 (italic ×3).
    let bold = Style::default().fg(Color::Red);
    let italic = Style::default().fg(Color::Green);
    let underline = Style::default().fg(Color::Blue);

    let line = Line::from(vec![
        Span::styled("AAA", bold),
        Span::raw(" "),
        Span::styled("BBB", italic),
        Span::raw(" "),
        Span::styled("CCC", underline),
    ]);

    let wrapped = wrap_line_to_visual_lines(&line, 4);
    assert_eq!(
        wrapped.len(),
        3,
        "textwrap at width 4 must split into 3 visual lines"
    );

    // Collect (char, style.fg) pairs from each visual line.
    let collect_fg = |l: &Line<'_>| -> Vec<Option<Color>> {
        l.spans
            .iter()
            .flat_map(|span| span.content.chars().map(move |_| span.style.fg))
            .collect()
    };

    // v0 = "AAA" — all red (bold).
    assert_eq!(
        collect_fg(&wrapped[0]),
        vec![Some(Color::Red), Some(Color::Red), Some(Color::Red)],
        "v0 must be entirely red"
    );
    // v1 = "BBB" — all green (italic). Before the fix, the first char
    // would inherit the previous span's style (None from the raw space)
    // or shift forward by one.
    assert_eq!(
        collect_fg(&wrapped[1]),
        vec![Some(Color::Green), Some(Color::Green), Some(Color::Green)],
        "v1 must be entirely green — proves global_char_offset was realigned"
    );
    // v2 = "CCC" — all blue (underline).
    assert_eq!(
        collect_fg(&wrapped[2]),
        vec![Some(Color::Blue), Some(Color::Blue), Some(Color::Blue)],
        "v2 must be entirely blue"
    );
}

#[test]
fn spawn_subagent_detail_parses_agent_id() {
    assert_eq!(
        parse_spawn_subagent_agent_id(r#"{"agent_id":"child-123"}"#),
        Some("child-123".to_string())
    );
}

#[test]
fn join_subagent_detail_parses_terminal_snapshot() {
    let parsed = parse_join_subagent_terminal(
        r#"{"terminal":{"status":"completed","reply":"done","error":null,"completed_at_ms":123}}"#,
    )
    .expect("join_subagent detail should parse");

    assert_eq!(parsed.status, "completed");
    assert_eq!(parsed.reply.as_deref(), Some("done"));
    assert_eq!(parsed.error, None);
    assert_eq!(parsed.completed_at_ms, Some(123));
}

#[test]
fn selection_highlight_preserves_wrapped_visual_layout() {
    let line = Line::from("  assistant output with enough text to wrap");
    let wrapped_before = wrap_line_to_visual_lines(&line.clone(), 12);
    let highlighted = highlight_line_selection(line, 4, 18, Style::default());
    let wrapped_after = wrap_line_to_visual_lines(&highlighted, 12);

    let before_text: Vec<String> = wrapped_before
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    let after_text: Vec<String> = wrapped_after
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();

    assert_eq!(before_text, after_text);
}

#[test]
fn file_edit_args_parse_display_fields() {
    let args = serde_json::json!({
        "file_path": "README.md",
        "old_string": "before\n",
        "new_string": "after\n",
        "replace_all": true
    })
    .to_string();

    let parsed = parse_file_edit_args(&args).expect("file_edit args should parse");

    assert_eq!(parsed.file_path, "README.md");
    assert_eq!(parsed.old_string, "before\n");
    assert_eq!(parsed.new_string, "after\n");
    assert!(parsed.replace_all);
}

#[test]
fn tool_output_renders_markdown_tables_when_expanded() {
    let theme = Theme::detect();
    let message = Message::tool_event(ToolExecutionUpdate {
        call_id: "call-1".to_string(),
        tool: "bash".to_string(),
        summary: String::new(),
        args_preview: String::new(),
        command_preview: None,
        command: None,
        detail: "| Name | Status |\n| --- | --- |\n| xiaoO | ready |".to_string(),
        status: ToolExecutionStatus::Completed,
        exit_code: Some(0),
        duration_ms: Some(10),
        file_change: None,
    });
    let mut tool = message
        .tool_state
        .clone()
        .expect("tool message should carry tool state");
    tool.expanded = true;

    let lines =
        render_tool_message_lines(&message, &tool, Color::Green, &theme, 80, 0, &mut vec![]);
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(text.iter().any(|line| line.contains("┌")));
    assert!(text.iter().any(|line| line.contains("xiaoO")));
    assert!(!text.iter().any(|line| line.contains("| --- | --- |")));
}

#[test]
fn bash_tool_header_includes_command_when_collapsed() {
    let theme = Theme::detect();
    let message = Message::tool_event(ToolExecutionUpdate {
        call_id: "call-1".to_string(),
        tool: "bash".to_string(),
        summary: String::new(),
        args_preview: serde_json::json!({
            "command": "cargo test -p xiaoo-endside",
            "timeout": 120000
        })
        .to_string(),
        command_preview: None,
        command: None,
        detail: "full output".to_string(),
        status: ToolExecutionStatus::Completed,
        exit_code: Some(0),
        duration_ms: Some(10),
        file_change: None,
    });
    let tool = message
        .tool_state
        .as_ref()
        .expect("tool message should carry tool state");

    let lines = render_tool_message_lines(&message, tool, Color::Green, &theme, 80, 0, &mut vec![]);
    let rendered_text = rendered_lines_text(&lines);

    assert!(rendered_text.contains("bash: cargo test -p xiaoo-endside  done"));
    assert!(!rendered_text.contains("Command"));
    assert!(!rendered_text.contains("full output"));
    assert!(!rendered_text.contains("timeout"));
}

#[test]
fn expanded_bash_tool_filters_timeout_and_decodes_escaped_output() {
    let theme = Theme::detect();
    let message = Message::tool_event(ToolExecutionUpdate {
        call_id: "call-1".to_string(),
        tool: "bash".to_string(),
        summary: String::new(),
        args_preview: serde_json::json!({
            "command": "printf 'a\\nb'",
            "cwd": "/tmp/work",
            "timeout": 120000
        })
        .to_string(),
        command_preview: None,
        command: None,
        detail: "line1\\nline2\\tindented".to_string(),
        status: ToolExecutionStatus::Completed,
        exit_code: Some(0),
        duration_ms: Some(10),
        file_change: None,
    });
    let mut tool = message
        .tool_state
        .clone()
        .expect("tool message should carry tool state");
    tool.expanded = true;

    let lines =
        render_tool_message_lines(&message, &tool, Color::Green, &theme, 80, 0, &mut vec![]);
    let rendered_text = rendered_lines_text(&lines);

    assert!(rendered_text.contains("Command"));
    assert!(rendered_text.contains("printf 'a"));
    assert!(rendered_text.contains("Arguments"));
    assert!(rendered_text.contains("\"cwd\": \"/tmp/work\""));
    assert!(!rendered_text.contains("\"timeout\""));
    assert!(rendered_text.contains("line1"));
    assert!(rendered_text.contains("line2\tindented"));
}

#[test]
fn expanded_tool_lines_use_subtle_background() {
    let theme = Theme::detect();
    let message = Message::tool_event(ToolExecutionUpdate {
        call_id: "call-1".to_string(),
        tool: "bash".to_string(),
        summary: String::new(),
        args_preview: serde_json::json!({
            "command": "date"
        })
        .to_string(),
        command_preview: None,
        command: None,
        detail: "Mon Jun 29".to_string(),
        status: ToolExecutionStatus::Completed,
        exit_code: Some(0),
        duration_ms: None,
        file_change: None,
    });
    let mut tool = message
        .tool_state
        .clone()
        .expect("tool message should carry tool state");

    let collapsed_lines =
        render_tool_message_lines(&message, &tool, Color::Green, &theme, 80, 0, &mut vec![]);
    assert!(collapsed_lines.iter().all(|line| line.style.bg.is_none()));

    tool.expanded = true;
    let expanded_lines =
        render_tool_message_lines(&message, &tool, Color::Green, &theme, 80, 0, &mut vec![]);
    let bg = Some(expanded_tool_background(&theme));
    assert_ne!(bg, Some(theme.background));
    assert_ne!(bg, Some(theme.assistant_message_bg));
    let panel_lines = expanded_lines
        .iter()
        .take_while(|line| line_display_width(line) > 0)
        .collect::<Vec<_>>();
    assert!(panel_lines.len() >= 4);
    assert!(panel_lines.iter().all(|line| line.style.bg == bg));
    assert!(line_display_width(panel_lines[0]) >= 80);
    assert!(line_display_width(panel_lines[panel_lines.len() - 1]) >= 80);
    assert!(rendered_line_text(panel_lines[0]).trim().is_empty());
    assert!(rendered_line_text(panel_lines[panel_lines.len() - 1])
        .trim()
        .is_empty());
}

#[test]
fn expanded_tool_panel_overrides_nested_markdown_backgrounds() {
    let theme = Theme::detect();
    let mut lines = vec![Line::from(vec![Span::styled(
        "`code`",
        Style::default().fg(theme.foreground).bg(theme.code_bg),
    )])];

    apply_expanded_tool_panel(&mut lines, &theme, 80);

    let bg = Some(expanded_tool_background(&theme));
    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .all(|span| span.style.bg.is_none() || span.style.bg == bg));
    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| span.content == "`code`" && span.style.bg == bg));
}

#[test]
fn tool_toggle_row_tracks_expanded_panel_spacer() {
    let theme = Theme::detect();
    let mut message = Message::tool_event(ToolExecutionUpdate {
        call_id: "call-1".to_string(),
        tool: "bash".to_string(),
        summary: String::new(),
        args_preview: serde_json::json!({
            "command": "date"
        })
        .to_string(),
        command_preview: None,
        command: None,
        detail: "ok".to_string(),
        status: ToolExecutionStatus::Completed,
        exit_code: Some(0),
        duration_ms: None,
        file_change: None,
    });

    let (collapsed, _state) = render_message_entry(&message, &theme, 80, 0, false, false, "", None);
    assert_eq!(collapsed.tool_toggle_row_offset, Some(1));

    message
        .tool_state
        .as_mut()
        .expect("tool state should exist")
        .expanded = true;
    let (expanded, _state) = render_message_entry(&message, &theme, 80, 0, false, false, "", None);
    assert_eq!(expanded.tool_toggle_row_offset, Some(2));
}

#[test]
fn side_by_side_diff_pairs_replacement_lines() {
    let rows = build_side_by_side_diff_rows("one\ntwo\nthree\n", "one\ndeux\nthree\n");
    let (additions, deletions) = diff_change_counts(&rows);

    assert_eq!((additions, deletions), (1, 1));
    let changed = rows
        .iter()
        .find(|row| row.left.as_ref().is_some_and(|side| side.text == "two"))
        .expect("replacement row should exist");
    assert_eq!(
        changed.right.as_ref().map(|side| side.text.as_str()),
        Some("deux")
    );
}

#[test]
fn file_edit_render_includes_path_and_stats() {
    let args = serde_json::json!({
        "file_path": "README.md",
        "old_string": "before\n",
        "new_string": "after\n"
    })
    .to_string();
    let edit = parse_file_edit_args(&args).expect("file_edit args should parse");
    let message = Message::tool_event(ToolExecutionUpdate {
        call_id: "call-1".to_string(),
        tool: "file_edit".to_string(),
        summary: String::new(),
        args_preview: args,
        command_preview: None,
        command: None,
        detail: String::new(),
        status: ToolExecutionStatus::Completed,
        exit_code: None,
        duration_ms: None,
        file_change: None,
    });
    let tool = message
        .tool_state
        .as_ref()
        .expect("tool message should carry tool state");

    let lines = render_file_edit_tool_lines(
        &message,
        tool,
        &edit,
        Color::Green,
        &Theme::detect(),
        80,
        0,
        &mut vec![],
    );
    let rendered_text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered_text.contains("Edit README.md"));
    assert!(rendered_text.contains("+1 -1"));
    assert!(rendered_text.contains("Original"));
    assert!(rendered_text.contains("Updated"));
}

fn rendered_lines_text(lines: &[Line<'static>]) -> String {
    lines
        .iter()
        .map(rendered_line_text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn rendered_line_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn make_bench_render(
    lines: &[Line<'static>],
    width: u16,
    cache_wrapped: bool,
) -> CachedMessageRender {
    let wrapped_lines = if cache_wrapped {
        Some(
            lines
                .iter()
                .map(|l| wrap_line_to_visual_lines(l, width))
                .collect(),
        )
    } else {
        None
    };
    CachedMessageRender {
        width,
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
        wrapped_lines,
        lines: lines.to_vec(),
        frozen_prefix_line_count: None,
    }
}

/// Build a `(renders_all_dirty, renders_partial)` pair for a bench scenario.
///
/// `renders_all_dirty`: every slot is `Some` (freshly rendered) — used to
/// build the prev cache and to measure the full-rebuild path.
/// `renders_partial`: only the last `dirty_count` slots are `Some`; the
/// rest are `None` so their blocks MOVE from the prev cache. This models a
/// streaming tick where only the tail (and possibly a few recently-settled
/// messages) changed.
fn make_bench_renders(
    num_messages: usize,
    content_lines: &[Line<'static>],
    width: u16,
    dirty_count: usize,
) -> (
    Vec<Option<CachedMessageRender>>,
    Vec<Option<CachedMessageRender>>,
) {
    let all_dirty: Vec<Option<CachedMessageRender>> = (0..num_messages)
        .map(|_| Some(make_bench_render(content_lines, width, true)))
        .collect();
    let dirty_tail_start = num_messages.saturating_sub(dirty_count);
    let partial: Vec<Option<CachedMessageRender>> = (0..num_messages)
        .map(|i| {
            if i >= dirty_tail_start {
                Some(make_bench_render(content_lines, width, true))
            } else {
                None
            }
        })
        .collect();
    (all_dirty, partial)
}

/// Time the three `build_transcript_cache` paths and return per-run µs:
/// `(full_no_prev, incremental_step, full_with_prev)`.
///
/// - `full_no_prev` (A): cold cache, every message dirty.
/// - `incremental_step` (B-A): pure incremental cost, isolated by
///   subtracting A from B (B includes building the prev cache each
///   iteration, which is A's cost, so B-A cancels it).
/// - `full_with_prev` (C): prev exists but every message is dirty
///   (width/theme-change style) — measures the `Option::take` overhead
///   on the non-moving path.
///
/// All three include the cost of cloning the renders vec each iteration
/// (`build_transcript_cache` takes it by value); this is a constant
/// bias across paths and scenarios, so ratios and trends remain valid.
fn time_bench_paths(
    all_dirty: &[Option<CachedMessageRender>],
    partial: &[Option<CachedMessageRender>],
    num_warmup: u32,
    num_measure: u32,
) -> (u128, u128, u128) {
    use std::time::Instant;

    for _ in 0..num_warmup {
        let _ = build_transcript_cache(None, all_dirty.to_vec());
        let prev = build_transcript_cache(None, all_dirty.to_vec());
        let _ = build_transcript_cache(Some(prev), partial.to_vec());
    }
    let a_start = Instant::now();
    for _ in 0..num_measure {
        let _ = build_transcript_cache(None, all_dirty.to_vec());
    }
    let a_us = a_start.elapsed().as_micros() / num_measure as u128;
    let b_start = Instant::now();
    for _ in 0..num_measure {
        let prev = build_transcript_cache(None, all_dirty.to_vec());
        let _ = build_transcript_cache(Some(prev), partial.to_vec());
    }
    let b_us = b_start.elapsed().as_micros() / num_measure as u128;
    let c_start = Instant::now();
    for _ in 0..num_measure {
        let prev = build_transcript_cache(None, all_dirty.to_vec());
        let _ = build_transcript_cache(Some(prev), all_dirty.to_vec());
    }
    let c_us = c_start.elapsed().as_micros() / num_measure as u128;
    (a_us, b_us.saturating_sub(a_us), c_us)
}

/// Deterministic equivalence check between an incremental rebuild (prev
/// cache + partial dirty renders) and a full rebuild of the same
/// messages: every flat index, background, and per-block `lines` /
/// `visual_lines` / offset table must be identical — and both caches
/// must satisfy the flat-index invariants (`assert_cache_invariants`).
///
/// This is the unit-testable correctness property of the incremental
/// path. It replaces wall-clock assertions: µs-scale `Instant`
/// measurements are inherently unreliable under parallel test load (a
/// single preemption in one measurement window but not the other can
/// invert any `incr < full` comparison), so timing is printed for humans
/// and never asserted.
fn assert_incremental_equivalent_to_full(
    all_dirty: &[Option<CachedMessageRender>],
    partial: &[Option<CachedMessageRender>],
    label: &str,
) {
    let full_cache = build_transcript_cache(None, all_dirty.to_vec());
    let prev_cache = build_transcript_cache(None, all_dirty.to_vec());
    let incr_cache = build_transcript_cache(Some(prev_cache), partial.to_vec());
    assert_caches_equivalent(&incr_cache, &full_cache, label);
    assert_cache_invariants(&incr_cache, label);
    assert_cache_invariants(&full_cache, label);
}

/// Flat-index bookkeeping invariants of a `TranscriptRenderCache`:
///
///  - blocks are contiguous: `block[i+1].start_visual_row` /
///    `logical_line_start` continue exactly where `block[i]` ended, and
///    `total_lines` equals the sum of all `visual_lines`;
///  - the flat logical indices (`logical_line_visual_starts`,
///    `line_texts`, `line_is_header`) have one entry per logical line,
///    and each global start equals `block.start_visual_row +
///    block.logical_to_visual_offset[j]`;
///  - per block, `logical_to_visual_offset` has one entry per logical
///    line, starts at 0, and is non-decreasing;
///  - `visual_line_backgrounds` has one entry per visual line;
///  - `line_texts[i]` is exactly the concatenated span text of logical
///    line `i` (the mouse/selection copy source).
///
/// These are the fields the incremental path rebuilds by walking moved
/// blocks, so a bookkeeping bug (wrong truncate/extend, shifted offsets,
/// stale text) breaks them even when a visual comparison would pass.
fn assert_cache_invariants(cache: &TranscriptRenderCache, label: &str) {
    let mut expect_visual_row = 0usize;
    let mut expect_logical_row = 0usize;
    for (i, block) in cache.message_blocks.iter().enumerate() {
        assert_eq!(
            block.start_visual_row, expect_visual_row,
            "{label}: block {i} start_visual_row must continue contiguously"
        );
        assert_eq!(
            block.logical_line_start, expect_logical_row,
            "{label}: block {i} logical_line_start must continue contiguously"
        );
        assert_eq!(
            block.logical_to_visual_offset.len(),
            block.lines.len(),
            "{label}: block {i} l2v length must match lines length"
        );
        if !block.lines.is_empty() {
            assert_eq!(
                block.logical_to_visual_offset[0], 0,
                "{label}: block {i} first logical line starts at visual row 0"
            );
        }
        for j in 1..block.logical_to_visual_offset.len() {
            assert!(
                block.logical_to_visual_offset[j] >= block.logical_to_visual_offset[j - 1],
                "{label}: block {i} l2v must be non-decreasing at line {j}"
            );
        }
        for (j, &local_visual_start) in block.logical_to_visual_offset.iter().enumerate() {
            let global_logical = block.logical_line_start + j;
            assert_eq!(
                cache.logical_line_visual_starts.get(global_logical),
                Some(&(block.start_visual_row + local_visual_start)),
                "{label}: block {i} logical line {j} global visual start"
            );
        }
        expect_visual_row += block.visual_lines.len();
        expect_logical_row += block.lines.len();
    }
    assert_eq!(
        cache.total_lines, expect_visual_row,
        "{label}: total_lines must equal the sum of block visual_lines"
    );
    assert_eq!(
        cache.logical_line_visual_starts.len(),
        expect_logical_row,
        "{label}: logical_line_visual_starts must have one entry per logical line"
    );
    assert_eq!(
        cache.line_texts.len(),
        expect_logical_row,
        "{label}: line_texts must have one entry per logical line"
    );
    assert_eq!(
        cache.line_is_header.len(),
        expect_logical_row,
        "{label}: line_is_header must have one entry per logical line"
    );
    assert_eq!(
        cache.visual_line_backgrounds.len(),
        cache.total_lines,
        "{label}: visual_line_backgrounds must have one entry per visual line"
    );
    for i in 1..cache.logical_line_visual_starts.len() {
        assert!(
            cache.logical_line_visual_starts[i] >= cache.logical_line_visual_starts[i - 1],
            "{label}: logical_line_visual_starts must be non-decreasing at {i}"
        );
    }
    for (i, text) in cache.line_texts.iter().enumerate() {
        let Some(line) = cache.logical_line(i) else {
            panic!("{label}: logical line {i} missing from blocks");
        };
        let joined: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(
            &joined, text,
            "{label}: line_texts[{i}] must match the block's logical line"
        );
    }
}

/// Field-wise equality of two `TranscriptRenderCache`s (the struct is
/// `Clone` but not `PartialEq`). Compares the flat indices plus every
/// block's content — a moved block must carry over byte-identical data.
fn assert_caches_equivalent(
    actual: &TranscriptRenderCache,
    expected: &TranscriptRenderCache,
    label: &str,
) {
    assert_eq!(
        actual.total_lines, expected.total_lines,
        "{label}: total_lines"
    );
    assert_eq!(
        actual.logical_line_visual_starts, expected.logical_line_visual_starts,
        "{label}: logical_line_visual_starts"
    );
    assert_eq!(
        actual.line_texts, expected.line_texts,
        "{label}: line_texts"
    );
    assert_eq!(
        actual.line_is_header, expected.line_is_header,
        "{label}: line_is_header"
    );
    assert_eq!(
        actual.visual_line_backgrounds, expected.visual_line_backgrounds,
        "{label}: visual_line_backgrounds"
    );
    assert_eq!(
        actual.message_blocks.len(),
        expected.message_blocks.len(),
        "{label}: message_blocks.len()"
    );
    for (i, (a, e)) in actual
        .message_blocks
        .iter()
        .zip(&expected.message_blocks)
        .enumerate()
    {
        assert_eq!(
            a.message_index, e.message_index,
            "{label}: block {i} message_index"
        );
        assert_eq!(
            a.start_visual_row, e.start_visual_row,
            "{label}: block {i} start_visual_row"
        );
        assert_eq!(
            a.logical_line_start, e.logical_line_start,
            "{label}: block {i} logical_line_start"
        );
        assert_eq!(
            a.logical_to_visual_offset, e.logical_to_visual_offset,
            "{label}: block {i} logical_to_visual_offset"
        );
        assert_eq!(
            a.tool_toggle_row_offset, e.tool_toggle_row_offset,
            "{label}: block {i} tool_toggle_row_offset"
        );
        assert_eq!(
            a.subagent_open_target
                .as_ref()
                .map(|t| (&t.agent_id, t.row_offset)),
            e.subagent_open_target
                .as_ref()
                .map(|t| (&t.agent_id, t.row_offset)),
            "{label}: block {i} subagent_open_target"
        );
        assert_eq!(a.lines, e.lines, "{label}: block {i} lines");
        assert_eq!(
            a.visual_lines, e.visual_lines,
            "{label}: block {i} visual_lines"
        );
    }
}

/// Short content fixture: each logical line fits in one visual line at
/// the bench widths, so wrapping is a no-op. Isolates the per-message /
/// per-logical-line overhead from the wrap cost.
fn short_content(lines_per_message: usize) -> Vec<Line<'static>> {
    (0..lines_per_message)
        .map(|i| Line::from(format!("Line {i}")))
        .collect()
}

/// Long content fixture: each logical line wraps to several visual lines
/// at width 80, exercising `wrap_line_to_visual_lines` and the flat-index
/// rebuild over a larger `visual_lines` vec.
fn long_content(lines_per_message: usize) -> Vec<Line<'static>> {
    (0..lines_per_message)
        .map(|i| {
            Line::from(format!(
                "Lorem ipsum dolor sit amet consectetur adipiscing elit adipiscing elit {i}"
            ))
        })
        .collect()
}

/// Microbenchmark for incremental `build_transcript_cache`.
///
/// Compares three paths and prints the timings (informational only — see
/// the note on wall-clock assertions below):
///  - A) Full rebuild, no prev (`None` for every message) — the
///    worst-case cost when the cache is cold (first tick / post-switch).
///  - B) Incremental rebuild: a prev cache exists, only the last message
///    is dirty, the rest move from prev. This is the streaming hot path.
///  - C) Full rebuild with a prev cache present (e.g. width change) —
///    prev is taken but every message is dirty, so nothing moves.
///
/// B includes the cost of building the prev cache each iteration (it is
/// consumed by `build_transcript_cache`), so `B - A` isolates the pure
/// incremental step. The deterministic assertions are:
///  - move-verification: non-dirty blocks share the same `visual_lines`
///    heap allocation as the prev cache (moved, not cloned);
///  - output-equivalence: the incremental rebuild is structurally
///    identical to a full rebuild of the same messages.
///
/// No wall-clock assertion is made: µs-scale `Instant` measurements are
/// inherently flaky under parallel test load (a scheduler preemption or
/// CPU-frequency step in one measurement window but not the other
/// inverts any `incr < full` comparison — the original timing assert
/// failed 11/12 runs under 12-way parallel `cargo test` load).
#[test]
fn build_transcript_cache_bench() {
    // Bench fixture: 50 messages × 5 logical lines, wrapped at width 80.
    let num_messages = 50usize;
    let lines_per_message = 5usize;
    // Terminal width used for wrapping throughout the bench.
    let bench_width: u16 = 80;
    // Warmup iterations to stabilise timings (JIT-less, but primes caches).
    let num_warmup_runs = 10u32;
    // Measured iterations averaged per path — large enough to dwarf the
    // `Instant::now()` overhead at µs granularity.
    let num_measure_runs = 100u32;

    let content_lines = long_content(lines_per_message);

    // Sanity: wrap_line_to_visual_lines works from test context.
    let test_wrapped = wrap_line_to_visual_lines(&content_lines[0], bench_width);
    assert!(
        !test_wrapped.is_empty(),
        "wrap_line_to_visual_lines should produce at least one visual line"
    );
    let test_miss = make_bench_render(&content_lines, bench_width, false);
    assert!(
        test_miss.wrapped_lines.is_none(),
        "cache_wrapped=false should produce None"
    );
    let test_hit = make_bench_render(&content_lines, bench_width, true);
    assert!(
        test_hit.wrapped_lines.is_some(),
        "cache_wrapped=true should produce Some(wrapped_lines)"
    );

    let (renders_all_dirty, renders_incremental) =
        make_bench_renders(num_messages, &content_lines, bench_width, 1);
    for (i, r) in renders_all_dirty.iter().enumerate() {
        assert!(r.is_some(), "render {i} should be Some (dirty)");
    }

    let (a_us, incremental_step_us, c_us) = time_bench_paths(
        &renders_all_dirty,
        &renders_incremental,
        num_warmup_runs,
        num_measure_runs,
    );

    // Move verification: a non-dirty block's `visual_lines` must keep the
    // same heap allocation across the incremental rebuild (moved, not
    // cloned). `Vec::as_ptr` is stable across moves of the owning Vec.
    // Block 0 is non-dirty (it is not the dirty tail), so it must be moved.
    let moved_block_idx = 0usize;
    let dirty_block_idx = num_messages - 1;
    let prev_for_check = build_transcript_cache(None, renders_all_dirty.clone());
    let moved_ptr = prev_for_check.message_blocks[moved_block_idx]
        .visual_lines
        .as_ptr();
    let new_cache = build_transcript_cache(Some(prev_for_check), renders_incremental.clone());
    assert_eq!(
        new_cache.message_blocks[moved_block_idx]
            .visual_lines
            .as_ptr(),
        moved_ptr,
        "incremental rebuild must move (not clone) non-dirty visual_lines — \
             pointer should be unchanged"
    );
    // The dirty (last) block, by contrast, is freshly built.
    assert_ne!(
        new_cache.message_blocks[dirty_block_idx]
            .visual_lines
            .as_ptr(),
        moved_ptr,
        "dirty block should be a fresh allocation"
    );

    eprintln!();
    eprintln!("=== build_transcript_cache microbenchmark (incremental) ===");
    eprintln!("  Messages: {num_messages}, ~{lines_per_message} lines each, width {bench_width}");
    eprintln!("  A) Full rebuild (no prev):       {a_us:>6} µs avg (over {num_measure_runs} runs)");
    eprintln!(
        "  B-A) Pure incremental step:      {incremental_step_us:>6} µs avg (1 dirty, prev exists)"
    );
    eprintln!("  C) Full rebuild with prev:       {c_us:>6} µs avg (over {num_measure_runs} runs, incl prev-build)");
    eprintln!(
        "  Speedup A vs (B-A):              {:.1}×",
        if incremental_step_us > 0 {
            a_us as f64 / incremental_step_us as f64
        } else {
            f64::INFINITY
        }
    );
    eprintln!();

    // Output equivalence: the incremental rebuild (prev cache + 1 dirty
    // tail) must be structurally identical to a full rebuild of the same
    // messages — the deterministic correctness property of the move path
    // (the pointer-identity checks above already prove the move itself).
    //
    // The µs timings printed above are informational ONLY. No wall-clock
    // assertion is made here: µs-scale `Instant` measurements are
    // inherently flaky under parallel test load (a scheduler preemption
    // or CPU-frequency step in one measurement window but not the other
    // inverts any `incr < full` comparison).
    assert_incremental_equivalent_to_full(&renders_all_dirty, &renders_incremental, "bench");
}

/// Deterministic matrix test for `build_transcript_cache`: for every
/// scenario, the incremental rebuild (prev cache + partial dirty
/// renders) must be structurally identical to a full rebuild of the
/// same messages, and both caches must satisfy the flat-index
/// invariants (see `assert_cache_invariants`).
///
/// Scenario axes (defaults: 50 messages, long content, width 80, 1
/// dirty tail):
///  1. Message count (0 / 10 / 50 / 200) at 1 dirty — 0 exercises the
///     empty transcript, 200 the largest flat-index rebuild.
///  2. Dirty count (1 / 10% / 50% / 100%) at 50 messages — 100%
///     exercises the "prev present but every message re-renders" path
///     (width-change worst case), where nothing moves but the prev
///     cache is still consumed.
///  3. Terminal width (40 / 80 / 120) at 50 messages, 1 dirty — wider
///     terminals wrap less, so fewer visual lines per message.
///  4. Content shape (short / long) at 50 messages, 1 dirty — short
///     content skips wrapping, isolating per-logical-line overhead.
///
/// This test deliberately contains NO timing. µs-scale wall-clock
/// assertions flake under parallel test load (the original timing-based
/// `build_transcript_cache_scalability` failed 11/12 runs under 12-way
/// parallel `cargo test`); timings for humans live in
/// `build_transcript_cache_bench`.
#[test]
fn build_transcript_cache_incremental_matches_full_matrix() {
    let default_messages = 50usize;
    let default_lines = 5usize;
    let default_width: u16 = 80;
    let default_dirty = 1usize;

    // Axis 1: message count, 1 dirty tail, long content, width 80.
    for &num_messages in &[0usize, 10usize, 50usize, 200usize] {
        let content = long_content(default_lines);
        let (all, partial) =
            make_bench_renders(num_messages, &content, default_width, default_dirty);
        let label = format!("msgs={num_messages} dirty=1");
        assert_incremental_equivalent_to_full(&all, &partial, &label);
    }

    // Axis 2: dirty count (tail), 50 messages, long content, width 80.
    for &dirty in &[1usize, 5usize, 25usize, 50usize] {
        let content = long_content(default_lines);
        let (all, partial) = make_bench_renders(default_messages, &content, default_width, dirty);
        let pct = (dirty * 100) / default_messages;
        let label = format!("msgs=50 dirty={dirty} ({pct}%)");
        assert_incremental_equivalent_to_full(&all, &partial, &label);
    }

    // Axis 3: terminal width, 50 messages, 1 dirty, long content.
    for &width in &[40u16, 80u16, 120u16] {
        let content = long_content(default_lines);
        let (all, partial) = make_bench_renders(default_messages, &content, width, default_dirty);
        let label = format!("msgs=50 dirty=1 w={width}");
        assert_incremental_equivalent_to_full(&all, &partial, &label);
    }

    // Axis 4: content shape, 50 messages, 1 dirty, width 80.
    for (label, content) in [
        ("short", short_content(default_lines)),
        ("long", long_content(default_lines)),
    ] {
        let (all, partial) =
            make_bench_renders(default_messages, &content, default_width, default_dirty);
        let label = format!("msgs=50 dirty=1 {label}");
        assert_incremental_equivalent_to_full(&all, &partial, &label);
    }
}

/// End-to-end test for the incremental block-move path in
/// `build_transcript_cache`: a dirty render with
/// `frozen_prefix_line_count = Some(n)` must move the first `n` logical
/// (and their visual) lines from the previous tick's block and append
/// the suffix, producing the same result as a full render.
#[test]
fn build_transcript_cache_incremental_moves_frozen_prefix() {
    let width: u16 = 80;

    // Previous tick: a full block [header, md1, md2, cursor, blank].
    // Each short line wraps to exactly one visual line. The Vecs are
    // constructed with spare capacity so the incremental path's
    // `truncate` + `extend` does not reallocate — letting us verify the
    // frozen prefix is MOVED (same heap allocation), not cloned.
    let mut prev_lines: Vec<Line<'static>> = Vec::with_capacity(32);
    prev_lines.push(Line::from("header"));
    prev_lines.push(Line::from("md1"));
    prev_lines.push(Line::from("md2"));
    prev_lines.push(Line::from("▌"));
    prev_lines.push(Line::raw(""));
    let mut prev_visuals: Vec<Line<'static>> = Vec::with_capacity(32);
    for l in &prev_lines {
        prev_visuals.extend(wrap_line_to_visual_lines(l, width));
    }
    let prev_l2v: Vec<usize> = {
        let mut v = Vec::new();
        let mut acc = 0;
        for l in &prev_lines {
            let w = wrap_line_to_visual_lines(l, width);
            v.push(acc);
            acc += w.len();
        }
        v
    };
    let line_texts: Vec<String> = prev_lines.iter().map(rendered_line_text).collect();
    let line_is_header: Vec<bool> = (0..prev_lines.len()).map(|i| i == 0).collect();
    let total_lines = prev_visuals.len();
    let visual_line_backgrounds = vec![None; total_lines];

    let prev_block = MessageVisualBlock {
        message_index: 0,
        start_visual_row: 0,
        logical_line_start: 0,
        lines: prev_lines,
        visual_lines: prev_visuals,
        logical_to_visual_offset: prev_l2v.clone(),
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
    };
    // Save the block's heap pointers before it is moved into the cache.
    // Vec::as_ptr is stable across moves and `truncate` (no realloc), so
    // the new block's moved prefix must share these exact allocations.
    let prev_lines_ptr = prev_block.lines.as_ptr();
    let prev_visuals_ptr = prev_block.visual_lines.as_ptr();
    let prev_cache = TranscriptRenderCache {
        message_blocks: vec![prev_block],
        logical_line_visual_starts: prev_l2v,
        line_texts,
        line_is_header,
        visual_line_backgrounds,
        total_lines,
    };

    // This tick: frozen prefix = [header, md1, md2] (3 lines).
    // Suffix = [md3, cursor, blank] (newly rendered).
    let suffix_lines: Vec<Line<'static>> = vec![Line::from("md3"), Line::from("▌"), Line::raw("")];
    let suffix_wrapped: Vec<Vec<Line<'static>>> = suffix_lines
        .iter()
        .map(|l| wrap_line_to_visual_lines(l, width))
        .collect();
    let render = CachedMessageRender {
        width,
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
        wrapped_lines: Some(suffix_wrapped),
        lines: suffix_lines,
        frozen_prefix_line_count: Some(3),
    };

    let new_cache = build_transcript_cache(Some(prev_cache), vec![Some(render)]);
    let block = &new_cache.message_blocks[0];

    // Combined logical lines = moved prefix + suffix.
    assert_eq!(block.lines.len(), 6, "prefix(3) + suffix(3)");
    assert_eq!(rendered_line_text(&block.lines[0]), "header");
    assert_eq!(rendered_line_text(&block.lines[1]), "md1");
    assert_eq!(rendered_line_text(&block.lines[2]), "md2");
    assert_eq!(rendered_line_text(&block.lines[3]), "md3");
    assert_eq!(rendered_line_text(&block.lines[4]), "▌");
    assert_eq!(rendered_line_text(&block.lines[5]), "");

    // The frozen prefix lines must be MOVED from the prev block (same
    // heap allocation), not cloned.
    assert_eq!(
        block.lines.as_ptr(),
        prev_lines_ptr,
        "frozen prefix lines must be moved (same allocation), not cloned"
    );
    assert_eq!(
        block.visual_lines.as_ptr(),
        prev_visuals_ptr,
        "frozen prefix visual_lines must be moved (same allocation)"
    );

    // logical_to_visual_offset: 6 logical lines, each → 1 visual line.
    assert_eq!(block.logical_to_visual_offset, vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(block.visual_lines.len(), 6);
    assert_eq!(new_cache.total_lines, 6);
}

/// A block-move where the suffix wraps to multiple visual lines must
/// produce correct `logical_to_visual_offset` entries that continue from
/// the frozen prefix's visual count (not restart at 0).
#[test]
fn build_transcript_cache_incremental_offset_continues_from_prefix() {
    let width: u16 = 10;

    // Prev: [header, long_line_that_wraps_to_2] → 3 visual lines total.
    let prev_lines: Vec<Line<'static>> =
        vec![Line::from("hdr"), Line::from("a very long line that wraps")];
    let prev_wrapped: Vec<Vec<Line<'static>>> = prev_lines
        .iter()
        .map(|l| wrap_line_to_visual_lines(l, width))
        .collect();
    let prev_visuals: Vec<Line<'static>> = prev_wrapped.iter().flatten().cloned().collect();
    let prev_l2v: Vec<usize> = {
        let mut v = Vec::new();
        let mut acc = 0;
        for wl in &prev_wrapped {
            v.push(acc);
            acc += wl.len();
        }
        v
    };
    let prefix_visual_count = prev_l2v[1]; // visual count for header only
    let prev_block = MessageVisualBlock {
        message_index: 0,
        start_visual_row: 0,
        logical_line_start: 0,
        lines: prev_lines.clone(),
        visual_lines: prev_visuals.clone(),
        logical_to_visual_offset: prev_l2v.clone(),
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
    };
    let prev_cache = TranscriptRenderCache {
        message_blocks: vec![prev_block],
        logical_line_visual_starts: prev_l2v.clone(),
        line_texts: prev_lines.iter().map(rendered_line_text).collect(),
        line_is_header: vec![true, false],
        visual_line_backgrounds: vec![None; prev_visuals.len()],
        total_lines: prev_visuals.len(),
    };

    // Freeze only the header (1 line); suffix = another long line.
    let suffix_lines: Vec<Line<'static>> = vec![Line::from("another long wrapping suffix line")];
    let suffix_wrapped: Vec<Vec<Line<'static>>> = suffix_lines
        .iter()
        .map(|l| wrap_line_to_visual_lines(l, width))
        .collect();
    let suffix_visual_count: usize = suffix_wrapped[0].len();
    let render = CachedMessageRender {
        width,
        wide_tables: Vec::new(),
        tool_toggle_row_offset: None,
        subagent_open_target: None,
        wrapped_lines: Some(suffix_wrapped),
        lines: suffix_lines,
        frozen_prefix_line_count: Some(1),
    };

    let new_cache = build_transcript_cache(Some(prev_cache), vec![Some(render)]);
    let block = &new_cache.message_blocks[0];

    assert_eq!(block.lines.len(), 2);
    assert_eq!(rendered_line_text(&block.lines[0]), "hdr");
    assert_eq!(
        rendered_line_text(&block.lines[1]),
        "another long wrapping suffix line"
    );

    // l2v[0] = 0 (header), l2v[1] = prefix_visual_count (suffix starts
    // after the header's visual lines).
    assert_eq!(block.logical_to_visual_offset.len(), 2);
    assert_eq!(block.logical_to_visual_offset[0], 0);
    assert_eq!(
        block.logical_to_visual_offset[1], prefix_visual_count,
        "suffix offset must continue from the frozen prefix's visual count"
    );
    assert_eq!(
        block.visual_lines.len(),
        prefix_visual_count + suffix_visual_count
    );
}

/// The incremental block-move path must merge wide-table metadata like it
/// merges `lines`/`visual_lines`: wide tables fully contained in the moved
/// frozen prefix are retained, partially-frozen tables are dropped (the
/// suffix re-renders their surviving part), and the suffix's freshly
/// rendered tables are appended — all with `start_line` coordinates already
/// rebased onto the full message block.
#[test]
fn build_transcript_cache_incremental_merges_wide_tables() {
    let width: u16 = 80;

    // Prev block: 6 logical lines, one visual line each.
    let prev_lines: Vec<Line<'static>> = (0..6).map(|i| Line::from(format!("p{i}"))).collect();
    let prev_visuals: Vec<Line<'static>> = prev_lines.clone();
    let prev_l2v: Vec<usize> = (0..6).collect();
    let prev_block = MessageVisualBlock {
        message_index: 0,
        start_visual_row: 0,
        logical_line_start: 0,
        lines: prev_lines,
        visual_lines: prev_visuals,
        logical_to_visual_offset: prev_l2v,
        wide_tables: vec![
            WideTableRegion {
                // Fully inside the frozen prefix (start 1 + count 2 <= freeze_n 4).
                start_line: 1,
                line_count: 2,
                natural_width: 40,
                horiz_offset: 0,
                viewport_width: 20,
            },
            // Spans past freeze_n (start 2 + count 3 > 4): not fully frozen.
            WideTableRegion {
                start_line: 2,
                line_count: 3,
                natural_width: 40,
                horiz_offset: 0,
                viewport_width: 20,
            },
        ],
        tool_toggle_row_offset: None,
        subagent_open_target: None,
    };
    let prev_cache = TranscriptRenderCache {
        message_blocks: vec![prev_block],
        logical_line_visual_starts: vec![0, 1, 2, 3, 4, 5],
        line_texts: (0..6).map(|i| format!("p{i}")).collect(),
        line_is_header: vec![true, false, false, false, false, false],
        visual_line_backgrounds: vec![None; 6],
        total_lines: 6,
    };

    // This tick: frozen prefix = first 4 lines; suffix = 3 new lines.
    let suffix_lines: Vec<Line<'static>> =
        vec![Line::from("s0"), Line::from("s1"), Line::from("s2")];
    let suffix_wrapped: Vec<Vec<Line<'static>>> = suffix_lines
        .iter()
        .map(|l| wrap_line_to_visual_lines(l, width))
        .collect();
    let render = CachedMessageRender {
        width,
        wide_tables: vec![WideTableRegion {
            // A table opened in the suffix; rebased onto the full block:
            // first suffix line sits at block index 4, so this starts at 5.
            start_line: 5,
            line_count: 3,
            natural_width: 50,
            horiz_offset: 8,
            viewport_width: 20,
        }],
        tool_toggle_row_offset: None,
        subagent_open_target: None,
        wrapped_lines: Some(suffix_wrapped),
        lines: suffix_lines,
        frozen_prefix_line_count: Some(4),
    };

    let new_cache = build_transcript_cache(Some(prev_cache), vec![Some(render)]);
    let block = &new_cache.message_blocks[0];

    assert_eq!(block.lines.len(), 7, "prefix(4) + suffix(3)");
    assert_eq!(
        block.wide_tables,
        vec![
            WideTableRegion {
                start_line: 1,
                line_count: 2,
                natural_width: 40,
                horiz_offset: 0,
                viewport_width: 20,
            },
            WideTableRegion {
                start_line: 5,
                line_count: 3,
                natural_width: 50,
                horiz_offset: 8,
                viewport_width: 20,
            },
        ],
        "fully-frozen table retained; partially-frozen dropped; suffix table appended"
    );
    assert_eq!(
        block.logical_to_visual_offset,
        vec![0, 1, 2, 3, 4, 5, 6],
        "l2v continues from frozen prefix into suffix"
    );
}

/// End-to-end: a wide markdown table rendered through `render_message_entry`
/// lands in `CachedMessageRender.wide_tables` with a `start_line` rebased
/// past the role header + thinking block, survives a full `build_transcript_cache`
/// pass unchanged, and stays windowed (each logical row <= viewport).
#[test]
fn wide_table_metadata_flows_through_message_render_and_cache() {
    let theme = Theme::detect();
    let content = "| A | B |\n| --- | --- |\n| a-very-long-value | another-long-value |";
    let mut message = Message::assistant_streaming();
    message.is_streaming = false;
    message.thinking_content = "think".to_string();
    message.content = content.to_string();

    // Wide-enough viewport: no metadata, full content visible.
    let (wide_enough, _state) =
        render_message_entry(&message, &theme, 80, 0, false, false, "", None);
    assert!(wide_enough.wide_tables.is_empty());

    // Narrow viewport: table is windowed; start_line rebased past header +
    // thinking block (1 header + 1 thinking header + 1 body + 1 blank = 4).
    let (render, _state) = render_message_entry(&message, &theme, 20, 0, false, false, "", None);
    assert_eq!(render.wide_tables.len(), 1);
    let wt = render.wide_tables[0];
    assert_eq!(wt.start_line, 4);
    assert!(wt.natural_width > 20);
    assert_eq!(wt.viewport_width, 20);
    assert_eq!(wt.horiz_offset, 0);
    // Every logical line of the windowed table (incl. hint) fits the viewport.
    for row in wt.start_line..(wt.start_line + wt.line_count) {
        assert!(
            line_display_width(&render.lines[row]) <= 20,
            "windowed row {row} exceeds viewport"
        );
    }

    // A huge offset clamps to the rightmost window.
    let (render_max, _state) =
        render_message_entry(&message, &theme, 20, 1000, false, false, "", None);
    let wt_max = render_max.wide_tables[0];
    assert_eq!(
        wt_max.horiz_offset,
        wt_max.natural_width - wt_max.viewport_width,
        "offset must clamp to the rightmost window"
    );

    // Building the cache carries the metadata into the block unchanged.
    let expected_line_4 = rendered_line_text(&render.lines[4]);
    let cache = build_transcript_cache(None, vec![Some(render)]);
    let block = &cache.message_blocks[0];
    assert_eq!(block.wide_tables.len(), 1);
    assert_eq!(block.wide_tables[0].start_line, 4);
    assert_eq!(
        rendered_line_text(&block.lines[4]),
        expected_line_4,
        "block logical lines must be identical to the render's"
    );
}
