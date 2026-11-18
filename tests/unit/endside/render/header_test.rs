use ratatui::style::{Color, Modifier};

use super::running_status_light;

#[test]
fn running_status_light_blinks_at_human_scale() {
    let (bright_symbol, bright_style) = running_status_light(0, Color::Green);
    let (dim_symbol, dim_style) = running_status_light(700, Color::Green);

    assert_eq!(bright_symbol, "●");
    assert!(bright_style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(dim_symbol, "○");
    assert!(!dim_style.add_modifier.contains(Modifier::BOLD));
}

// ── Status bar: Think(Ctrl+T) display contract ──────────────────────────────
//
// The E2E suites assert on the rendered status bar text
// (`Think(Ctrl+T) <value>`), so the exact label and the per-state value
// strings are a public contract. `default` is a state of its own (distinct
// from `off`), and it renders as "default" — these tests pin that down at
// the layer the E2E tests actually observe.

use crate::app::App;
use crate::config::Config;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use xiaoo_api::chat::ReasoningEffort;

/// Flatten the rendered buffer back into text (same wide-char handling as
/// the overlay tests: skip the padding cell after a CJK glyph).
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
                x += ratatui::text::Line::from(symbol).width().max(1) as u16;
            }
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn status_bar_test_app() -> App {
    let mut config = Config::default();
    config.llm.provider = "openai".to_string();
    config.llm.model = "pinned-model".to_string();
    App::new_with_config(
        &config,
        PathBuf::from("/nonexistent/endside-header-test-config.toml"),
        PathBuf::from("/tmp/endside-header-test-workspace"),
        None,
    )
    .expect("headless test app")
}

#[tokio::test]
async fn status_bar_renders_reasoning_effort_contract() {
    let mut app = status_bar_test_app();
    let mut terminal = Terminal::new(TestBackend::new(180, 3)).expect("test terminal");

    // Every state renders its own value right after the label.
    for (effort, label) in [
        (ReasoningEffort::Default, "default"),
        (ReasoningEffort::Off, "off"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::Max, "max"),
    ] {
        app.state.reasoning_effort = effort;
        terminal
            .draw(|frame| app.render_status_bar(frame, frame.area()))
            .expect("status bar draw");
        let text = screen_text(&terminal);
        assert!(
            text.contains(&format!("Think(Ctrl+T) {label}")),
            "status bar must render 'Think(Ctrl+T) {label}' for {effort:?}, got:\n{text}"
        );
    }

    // Cycling must be visible in the rendered bar: from `default` the first
    // Ctrl+T lands on `off`, not `high` (four-state cycle) — the exact
    // behavior E2E suites observe when pressing the key.
    app.state.reasoning_effort = ReasoningEffort::Default;
    app.state.cycle_reasoning_effort();
    terminal
        .draw(|frame| app.render_status_bar(frame, frame.area()))
        .expect("status bar draw");
    let text = screen_text(&terminal);
    assert!(
        text.contains("Think(Ctrl+T) off"),
        "first Ctrl+T from default must render off, got:\n{text}"
    );
    assert!(
        !text.contains("Think(Ctrl+T) high"),
        "first Ctrl+T from default must not render high, got:\n{text}"
    );
}
