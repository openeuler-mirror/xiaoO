use super::{extract_context_length_from_raw, find_model_summary, ModelSummary};

#[test]
fn find_model_summary_matches_gemini_prefixed_names() {
    let models = vec![ModelSummary::new("gemini-2.5-pro")];
    let found = find_model_summary(&models, "models/gemini-2.5-pro");
    assert!(found.is_some());
}

#[test]
fn extract_context_length_from_raw_finds_max_input_tokens() {
    let raw = serde_json::json!({
        "id": "claude-sonnet",
        "max_input_tokens": 200000
    });

    assert_eq!(extract_context_length_from_raw(&raw), Some(200000));
}
