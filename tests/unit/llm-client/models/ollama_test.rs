use super::find_context_length_in_value;

#[test]
fn finds_ollama_model_info_context_length() {
    let body = serde_json::json!({
        "model_info": {
            "llama.context_length": 32768
        }
    });

    assert_eq!(find_context_length_in_value(&body, None), Some(32768));
}
