use super::truncate_json_for_display;
use serde_json::json;

fn chinese_extras() -> serde_json::Value {
    // Mirrors a real LLM_CALL span: 118 bytes rendered compactly, with a
    // Chinese task text whose bytes straddle the 77-byte cut used by
    // `export --format markdown`.
    json!({
        "agent_id": "main",
        "effective_request": {
            "messages": [{"blocks": [{"text": "修复参数错误并验证边界处理"}]}]
        }
    })
}

#[test]
fn ascii_output_is_unchanged() {
    let value = json!({
        "agent_id": "main",
        "text": "fix param error and verify boundary handling across the whole call chain"
    });
    let rendered = value.to_string();
    assert!(
        rendered.len() > 80,
        "sample must exceed the budget: {}",
        rendered.len()
    );

    let truncated = truncate_json_for_display(&value, 80);
    assert_eq!(truncated.len(), 80);
    assert_eq!(truncated, format!("{}...", &rendered[..77]));
    assert!(truncated.ends_with("..."));
}

#[test]
fn short_values_are_not_truncated() {
    let value = json!({"ok": true});
    assert_eq!(truncate_json_for_display(&value, 80), value.to_string());
}

/// A budget smaller than the three-dot marker cannot fit any payload: the
/// result is the marker alone. (The old implementation underflowed
/// `max_len - 3` here and panicked in debug builds.)
#[test]
fn budgets_below_the_marker_return_only_the_marker() {
    let value = json!({"ok": true});
    assert_eq!(truncate_json_for_display(&value, 0), "...");
    assert_eq!(truncate_json_for_display(&value, 2), "...");
}

/// The regression guard: this is the case that panicked at
/// `moirai.rs:318` (`end byte index 77 is not a char boundary; it is inside
/// '复' (bytes 75..78)`).
#[test]
fn chinese_extras_do_not_panic_at_the_export_budget() {
    let value = chinese_extras();
    let rendered = value.to_string();
    assert!(
        rendered.len() > 80,
        "sample must exceed the budget: {}",
        rendered.len()
    );

    let truncated = truncate_json_for_display(&value, 80);
    assert!(
        truncated.len() <= 80,
        "budget must hold: {}",
        truncated.len()
    );
    assert!(truncated.ends_with("..."));
    assert!(rendered.starts_with(truncated.trim_end_matches('…').trim_end_matches("...")));
}

/// The second call site (`print_log`) uses a 40-byte budget; with Chinese text
/// early in the payload the cut at 37 bytes used to panic too
/// (`end byte index 37 ... it is inside '并' (bytes 35..38)`).
#[test]
fn chinese_extras_do_not_panic_at_the_log_budget() {
    let value = json!({"说明": "修复参数错误并验证边界处理", "n": 1});
    let rendered = value.to_string();
    assert!(
        rendered.len() > 40,
        "sample must exceed the budget: {}",
        rendered.len()
    );

    let truncated = truncate_json_for_display(&value, 40);
    assert!(truncated.len() <= 40);
    assert!(truncated.ends_with("..."));
}

#[test]
fn every_budget_is_a_safe_cut_point() {
    let samples = [
        chinese_extras(),
        json!({"说明": "修复参数错误并验证边界处理"}),
        json!({"emoji": "😀🚀 与中文"}),
        json!({"ascii": "plain ascii payload that is long enough to be cut"}),
    ];
    for value in samples {
        let rendered = value.to_string();
        for budget in 0..=rendered.len() + 4 {
            let truncated = truncate_json_for_display(&value, budget);
            // Only guarantee: no panic, and the kept prefix is a real prefix.
            if budget >= 3 && rendered.len() > budget {
                assert!(truncated.ends_with("..."));
                let kept = truncated.trim_end_matches("...");
                assert!(rendered.starts_with(kept), "kept part must be a prefix");
                assert!(kept.len() + 3 <= budget);
            }
        }
    }
}
