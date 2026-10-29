use super::*;
use crate::app::App;
use crate::app_state::InputMode;
use crate::chat::{ModelInfo, ProviderInfo};
use crate::config::Config;
use crate::provider_dialog::ProviderDialog;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Terminal;
use std::path::PathBuf;

#[test]
fn cursor_position_tracks_plain_text() {
    let (row, col) = calculate_visual_cursor_position("hello", 3, 10);
    assert_eq!((row, col), (0, 3));
}

#[test]
fn cursor_position_resets_column_after_newline() {
    let (row, col) = calculate_visual_cursor_position("hello\nworld", 6, 10);
    assert_eq!((row, col), (1, 0));
}

#[test]
fn cursor_position_at_trailing_newline_starts_next_row() {
    let (row, col) = calculate_visual_cursor_position("hello\n", 6, 10);
    assert_eq!((row, col), (1, 0));
}

#[test]
fn cursor_position_of_empty_input_is_origin() {
    let (row, col) = calculate_visual_cursor_position("", 0, 10);
    assert_eq!((row, col), (0, 0));
}

#[test]
fn cursor_position_wraps_after_trailing_spaces() {
    // "aa   bb" at width 4: ratatui renders "aa  " / "bb  " (two trailing
    // spaces fill the first line as background). Cursor at the end (7) must
    // be on row 1 at the end of "bb" (col 2).
    assert_eq!(calculate_visual_cursor_position("aa   bb", 7, 4), (1, 2));
    // cursor before the first 'b' lands at the start of row 1.
    assert_eq!(calculate_visual_cursor_position("aa   bb", 5, 4), (1, 0));
}

#[test]
fn cursor_position_breaks_long_words_at_width() {
    // "aaaaaaaa" at width 4 wraps as "aaaa" / "aaaa"; cursor=4 is the start
    // of the second line, not the (off-screen) end of the first.
    assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 4, 4), (1, 0));
    assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 8, 4), (1, 4));
}

#[test]
fn cursor_position_counts_cjk_as_double_width() {
    // Each CJK char is width 2; width 6 fits three per line.
    assert_eq!(calculate_visual_cursor_position("中文测试", 3, 6), (1, 0));
    assert_eq!(calculate_visual_cursor_position("中文测试", 4, 6), (1, 2));
}

/// Differential test: for every cursor offset, the (row, col) returned
/// must match the cell where ratatui's `Paragraph` + `Wrap { trim: false }`
/// actually renders that character (off-screen columns are skipped, since
/// the renderer clamps them separately).
fn ratatui_cell(value: &str, width: u16, row: usize, col: usize) -> String {
    let area = Rect {
        x: 0,
        y: 0,
        width,
        height: 80,
    };
    let mut buf = Buffer::empty(area);
    Widget::render(
        Paragraph::new(value).wrap(Wrap { trim: false }),
        area,
        &mut buf,
    );
    buf[(col as u16, row as u16)].symbol().to_string()
}

fn assert_matches_ratatui(value: &str, width: usize) {
    let chars: Vec<char> = value.chars().collect();
    for p in 0..=chars.len() {
        let (row, col) = calculate_visual_cursor_position(value, p, width);
        assert!(
            col <= width,
            "value={value:?} width={width} cursor={p}: col {col} > width {width}"
        );
        if p < chars.len() {
            let ch = chars[p];
            if ch == '\n' {
                continue;
            }
            let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if w == 0 || col == width {
                // zero-width chars and exact-fill boundary cursors have no
                // unique on-screen cell to compare against.
                continue;
            }
            let cell = ratatui_cell(value, width as u16, row, col);
            let cell_char = cell.chars().next();
            assert_eq!(
                cell_char,
                Some(ch),
                "value={value:?} width={width} cursor={p} (char {ch:?}): \
                     calc=({row},{col}) but ratatui cell is {cell:?}"
            );
        }
    }
}

#[test]
fn diff_basic_word() {
    assert_matches_ratatui("hello", 10);
}

#[test]
fn diff_long_single_word_wraps_mid_word() {
    assert_matches_ratatui("aaaaaaaa", 4);
}

#[test]
fn diff_word_wrap_with_spaces() {
    assert_matches_ratatui("aa bb cc dd", 4);
}

#[test]
fn diff_multiple_spaces_boundary() {
    assert_matches_ratatui("aa   bb", 4);
}

#[test]
fn diff_cursor_in_trailing_whitespace() {
    assert_matches_ratatui("aaaa ", 4);
}

#[test]
fn diff_newline_then_wrap() {
    assert_matches_ratatui("hello\naaaaaaa", 4);
}

#[test]
fn diff_cjk_wrap() {
    assert_matches_ratatui("中文测试一二三四五六", 6);
}

#[test]
fn diff_spaces_then_newline() {
    assert_matches_ratatui("aa  \nbb", 4);
}

#[test]
fn diff_long_paragraph() {
    assert_matches_ratatui(
        "The quick brown fox jumps over the lazy dog and keeps going",
        12,
    );
}

#[test]
fn diff_consecutive_newlines() {
    assert_matches_ratatui("a\n\nb", 4);
}

// ── Provider/model dialog: page height measured at render time ─────────────

/// Flatten the whole screen back into text, to assert on the *visible*
/// window rather than internal state.
///
/// The cell after a wide (CJK) character is the padding cell ratatui's
/// `Buffer::set_stringn` resets (its symbol is a space); skip it like a
/// real terminal does, otherwise a two-CJK-character word reads back with
/// a spurious space in the middle and every CJK assertion would fail.
fn screen_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut x = 0;
            while x < area.width {
                let symbol = buffer[(x, y)].symbol();
                line.push_str(symbol);
                x += Line::from(symbol).width().max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Put one provider + `count` `model-XX` entries into the dialog.
fn app_with_models(count: usize) -> App {
    let mut config = Config::default();
    config.llm.provider = "openrouter".to_string();
    config.llm.model = "pinned-model".to_string();
    let mut app = App::new_with_config(
        &config,
        PathBuf::from("/nonexistent/endside-overlay-test-config.toml"),
        PathBuf::from("/tmp/endside-overlay-test-workspace"),
        None,
    )
    .expect("headless test app");
    // The dialog only receives keys in this mode; this case renders
    // without dispatching keys, and this line exists purely to keep the
    // state identical to real usage (the layer that later implements
    // paging will press real keys here).
    app.state.input_mode = InputMode::ProviderSelection;
    app.state.provider_dialog = Some(ProviderDialog::new(vec![ProviderInfo {
        name: "openrouter".to_string(),
        models: (0..count)
            .map(|index| ModelInfo {
                id: format!("model-{index:02}"),
                name: format!("model-{index:02}"),
            })
            .collect(),
    }]));
    app
}

/// The visible rows measured at render time must equal the dialog's list
/// content area — that value *is* the definition of "one page". Here 14 =
/// dialog height 20 (42-row terminal) − 2 dialog borders − 2 (status +
/// hint rows) − 2 (the two column borders).
#[tokio::test]
async fn rendering_measures_the_list_viewport() {
    let mut app = app_with_models(40);
    let mut terminal = Terminal::new(TestBackend::new(130, 42)).expect("test terminal");

    terminal.draw(|frame| app.ui(frame)).expect("first draw");
    assert_eq!(
        app.state.provider_dialog.as_ref().unwrap().page_height,
        14,
        "渲染必须把一屏行数写回弹窗状态"
    );

    let first = screen_text(&terminal);
    assert!(first.contains("model-00"), "首屏从第一项开始");
    assert!(!first.contains("model-14"), "首屏放不下第二屏的内容");
}

#[test]
fn input_controls_render_without_terminal_control_bytes() {
    let normal = Style::default();
    let selected = Style::default().bg(ratatui::style::Color::Blue);
    let text = build_input_text_with_selection("A\t中\r\nB", &(1..4), normal, selected);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 3));
    Widget::render(
        Paragraph::new(text).wrap(Wrap { trim: false }),
        buffer.area,
        &mut buffer,
    );
    assert_eq!(buffer[(0, 0)].symbol(), "A");
    for col in 1..5 {
        assert_eq!(buffer[(col, 0)].symbol(), " ");
        assert_eq!(buffer[(col, 0)].bg, ratatui::style::Color::Blue);
    }
    assert_eq!(buffer[(5, 0)].symbol(), "中");
    assert_eq!(buffer[(7, 0)].symbol(), " ");
    assert_eq!(buffer[(0, 1)].symbol(), "B");
    assert!(buffer
        .content
        .iter()
        .all(|cell| !cell.symbol().chars().any(char::is_control)));
}

#[test]
fn preserved_controls_cursor_matches_expanded_paragraph_across_wraps() {
    for value in ["A\tB", "中\t文\rX\nY", "\tA\t", "A\r\nB"] {
        let display = value.replace('\t', "    ").replace('\r', " ");
        for width in [2, 4, 7, 20] {
            let mut offset = 0;
            for (index, ch) in value.chars().enumerate() {
                assert_eq!(
                    calculate_visual_cursor_position(value, index, width),
                    calculate_visual_cursor_position(&display, offset, width)
                );
                offset += if ch == '\t' { 4 } else { 1 };
            }
            assert_eq!(
                calculate_visual_cursor_position(value, value.chars().count(), width),
                calculate_visual_cursor_position(&display, offset, width)
            );
            assert_matches_ratatui(&display, width);
        }
    }
}
