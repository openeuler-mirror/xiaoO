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
    let result2 = render_markdown_incremental(Some(result.new_state), "single line", &theme, 40, 0);
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
