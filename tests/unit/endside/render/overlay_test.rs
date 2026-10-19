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
fn test_basic() {
    let (row, col) = calculate_visual_cursor_position("hello", 3, 10);
    assert_eq!((row, col), (0, 3));
}

#[test]
fn test_newline() {
    let (row, col) = calculate_visual_cursor_position("hello\nworld", 6, 10);
    assert_eq!((row, col), (1, 0));
}

#[test]
fn test_cursor_after_newline() {
    let (row, col) = calculate_visual_cursor_position("hello\n", 6, 10);
    assert_eq!((row, col), (1, 0));
}

#[test]
fn test_empty() {
    let (row, col) = calculate_visual_cursor_position("", 0, 10);
    assert_eq!((row, col), (0, 0));
}

#[test]
fn test_multiple_spaces_wrap() {
    // "aa   bb" at width 4: ratatui renders "aa  " / "bb  " (two trailing
    // spaces fill the first line as background). Cursor at the end (7) must
    // be on row 1 at the end of "bb" (col 2).
    assert_eq!(calculate_visual_cursor_position("aa   bb", 7, 4), (1, 2));
    // cursor before the first 'b' lands at the start of row 1.
    assert_eq!(calculate_visual_cursor_position("aa   bb", 5, 4), (1, 0));
}

#[test]
fn test_long_word_break() {
    // "aaaaaaaa" at width 4 wraps as "aaaa" / "aaaa"; cursor=4 is the start
    // of the second line, not the (off-screen) end of the first.
    assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 4, 4), (1, 0));
    assert_eq!(calculate_visual_cursor_position("aaaaaaaa", 8, 4), (1, 4));
}

#[test]
fn test_cjk_wrap() {
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

// ── provider / model 弹窗：渲染时量出的一屏行数 ─────────────────────────────

/// 把整屏还原成文本，用来断言"当前可见的窗口"而不是内部状态。
///
/// 宽字符（CJK）后面的那个 cell 是 ratatui `Buffer::set_stringn` 重置出来的
/// 填充格（symbol 是空格），按真实终端那样跳过它，否则 "翻页" 会被读成
/// "翻 页"、中文断言全部失效。
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

/// 弹窗里放一条 provider + `count` 个 `model-XX`。
fn app_with_models(count: usize) -> App {
    let mut config = Config::default();
    config.llm.provider = "openrouter".to_string();
    config.llm.model = "pinned-model".to_string();
    let mut app = App::new_with_config(
        &config,
        PathBuf::from("/nonexistent/endside-overlay-test-config.toml"),
        PathBuf::from("/tmp/endside-overlay-test-workspace"),
    )
    .expect("headless test app");
    // 弹窗只在该模式下接收按键；本用例只渲染、不派发按键，这行是为了让状态
    // 与真实使用时一致（后续实现翻页的那一层会在这里真的按键）。
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

/// 渲染时量出的可见行数必须等于弹窗里列表内容区的实际行数——这个值就是
/// "一屏"的定义。这里的 14 = 弹窗高 20（42 行终端）− 2 边框 − 2（状态行/
/// 快捷键行）− 2（左右两栏各自的边框）。
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
