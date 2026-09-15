use super::*;

fn plan(content: &str) -> TodoMessageState {
    TodoMessageState {
        title: "Implement plan panel scrolling".to_string(),
        items: vec![(TodoDisplayStatus::Pending, content.to_string())],
    }
}

/// Joined text of a rendered row (all spans concatenated).
fn row_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.to_string()).collect()
}

#[test]
fn short_item_fits_on_one_row_with_marker() {
    let rows = plan_item_visual_lines("✓", Color::Green, Color::Gray, "Fix login bug", 30);
    assert_eq!(rows.len(), 1);
    assert_eq!(row_text(&rows[0]), "[✓] Fix login bug");
}

#[test]
fn long_item_wraps_instead_of_truncating() {
    let content = "Refactor the authentication flow so that login failures retry with backoff and surface a clear user-facing error message after exhausting the retries";
    let rows = plan_item_visual_lines(" ", Color::DarkGray, Color::Gray, content, 30);
    assert!(
        rows.len() > 1,
        "long content must wrap onto multiple rows, got {}",
        rows.len()
    );
    // No ellipsis truncation: every character of the content is still
    // present across the wrapped rows (whitespace-insensitive compare).
    let rendered: String = rows
        .iter()
        .map(|l| row_text(l))
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let expected: String = format!("[ ] {content}")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert_eq!(rendered, expected);
}

#[test]
fn wrapped_continuation_rows_are_indented_under_content() {
    let content = "one two three four five six seven eight nine ten eleven twelve thirteen";
    let rows = plan_item_visual_lines(">", Color::Cyan, Color::Gray, content, 24);
    assert!(rows.len() >= 2, "content must wrap at width 24");
    let first = row_text(&rows[0]);
    assert!(first.starts_with("[>] "));
    let indent = " ".repeat(display_width(&sanitize_terminal_text("[>] ")));
    for continuation in &rows[1..] {
        let text = row_text(continuation);
        assert!(
            text.starts_with(&indent),
            "continuation row must be indented under the item text, got {text:?}"
        );
    }
}

#[test]
fn wrapped_rows_never_exceed_content_width() {
    let content = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let rows = plan_item_visual_lines("✓", Color::Green, Color::Gray, content, 30);
    assert!(rows.len() > 1);
    for row in &rows {
        let width: usize = row.spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(
            width <= 30,
            "row wider than the content area: {width} > 30 ({:?})",
            row_text(row)
        );
    }
}

#[test]
fn empty_item_still_renders_marker_row() {
    let rows = plan_item_visual_lines(" ", Color::DarkGray, Color::Gray, "", 30);
    assert_eq!(rows.len(), 1);
    // The prefix keeps its trailing space (marker column layout).
    assert_eq!(row_text(&rows[0]), "[ ] ");
}

#[test]
fn plan_panel_height_grows_for_wrapped_items() {
    let long = plan(
        "Refactor the authentication flow so that login failures retry with backoff and surface a clear user-facing error message",
    );
    // A narrower sidebar wraps the same item onto more rows, so the
    // panel must reserve more height.
    let narrow = plan_panel_height(&long, 28, 40);
    let wide = plan_panel_height(&long, 60, 40);
    assert!(
        narrow > wide,
        "narrower sidebar must reserve more rows: {narrow} vs {wide}"
    );
    // Both stay within the half-height cap.
    assert!(narrow <= 20, "height must respect the half-body cap");
}

#[test]
fn plan_panel_height_respects_bounds() {
    // A short list reserves exactly the 7-row minimum: borders(2) +
    // header(1) + blank(1) + 3 single-row items.
    let small = TodoMessageState {
        title: "t".to_string(),
        items: vec![
            (TodoDisplayStatus::Pending, "a".to_string()),
            (TodoDisplayStatus::Pending, "b".to_string()),
            (TodoDisplayStatus::Pending, "c".to_string()),
        ],
    };
    assert_eq!(plan_panel_height(&small, 30, 40), 7);

    // Many items exceed half of 40 → capped at 20.
    let big = TodoMessageState {
        title: "t".to_string(),
        items: (0..30)
            .map(|i| (TodoDisplayStatus::Pending, format!("task {i}")))
            .collect(),
    };
    assert_eq!(plan_panel_height(&big, 30, 40), 20);

    // Tiny available height passes through unchanged.
    assert_eq!(plan_panel_height(&big, 30, 10), 10);
}

#[test]
fn plan_panel_height_never_exceeds_actual_wrapped_content() {
    // Height prediction must equal what the renderer draws: borders +
    // header + blank + wrapped rows (unclamped when below the cap).
    let p = plan("a short item");
    // 2 borders + 1 header + 1 blank + 1 item row = 5 → clamped to min 7.
    assert_eq!(plan_panel_height(&p, 30, 40), 7);
    // With 8 one-row items: 2 + 1 + 1 + 8 = 12 rows.
    let p8 = TodoMessageState {
        title: "t".to_string(),
        items: (0..8)
            .map(|i| (TodoDisplayStatus::Pending, format!("item {i}")))
            .collect(),
    };
    assert_eq!(plan_panel_height(&p8, 60, 80), 12);
}
