use super::*;

fn providers() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            name: "openai".to_string(),
            models: vec![
                ModelInfo {
                    id: "gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                },
                ModelInfo {
                    id: "gpt-4-turbo".to_string(),
                    name: "GPT-4 Turbo".to_string(),
                },
            ],
        },
        ProviderInfo {
            name: "deepseek".to_string(),
            models: vec![
                ModelInfo {
                    id: "deepseek-v4-flash".to_string(),
                    name: "DeepSeek V4 Flash".to_string(),
                },
                ModelInfo {
                    id: "deepseek-v4-pro".to_string(),
                    name: "DeepSeek V4 Pro".to_string(),
                },
                ModelInfo {
                    id: "deepseek-chat".to_string(),
                    name: "DeepSeek Chat V3".to_string(),
                },
            ],
        },
    ]
}

#[test]
fn new_with_selection_preselects_matching_provider_and_model() {
    let dialog =
        ProviderDialog::new_with_selection(providers(), Some("openai"), Some("gpt-4-turbo"));

    assert_eq!(dialog.selected_provider, 0);
    assert_eq!(dialog.selected_model, 1);
    assert_eq!(
        dialog.selected(),
        Some(("openai".to_string(), "gpt-4-turbo".to_string()))
    );
}

#[test]
fn new_with_selection_falls_back_to_first_model_when_model_missing() {
    let dialog =
        ProviderDialog::new_with_selection(providers(), Some("deepseek"), Some("missing-model"));

    assert_eq!(dialog.selected_provider, 1);
    assert_eq!(dialog.selected_model, 0);
    assert_eq!(
        dialog.selected(),
        Some(("deepseek".to_string(), "deepseek-v4-flash".to_string()))
    );
}
