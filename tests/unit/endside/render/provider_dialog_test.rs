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

/// Pressing down repeatedly must stop at the last entry. Going out of
/// bounds makes `selected()` return `None`, and the apply-selection arm only
/// takes effect on `Some` — the UI shows nothing wrong, hence this test
/// pins both ends.
#[test]
fn move_down_in_the_models_pane_stops_at_the_last_model() {
    let mut dialog = ProviderDialog::new(providers());
    dialog.switch_to_models();

    for _ in 0..5 {
        dialog.move_down();
    }

    assert_eq!(
        dialog.selected_model, 1,
        "openai 只有 2 个模型，连按向下应停在末项而不是越过列表末尾"
    );
    assert!(
        dialog.selected().is_some(),
        "选中项必须落在列表范围内（越界会让 Enter 静默失效）"
    );
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

// ── Official-list replacement (replace_models) ──────────────────────────────

fn model(id: &str) -> ModelInfo {
    ModelInfo {
        id: id.to_string(),
        name: id.to_string(),
    }
}

fn ids(models: &[ModelInfo]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

#[test]
fn selected_provider_name_returns_none_when_empty() {
    let empty = ProviderDialog::new(Vec::new());
    assert_eq!(empty.selected_provider_name(), None);

    let dialog = ProviderDialog::new_with_selection(providers(), Some("deepseek"), None);
    assert_eq!(dialog.selected_provider_name(), Some("deepseek"));
}

/// The official list is ordered differently from the builtin table, so the
/// selection must persist by **id**, never by index.
#[test]
fn replace_models_keeps_selection_by_id() {
    let mut dialog =
        ProviderDialog::new_with_selection(providers(), Some("openai"), Some("gpt-4-turbo"));
    assert_eq!(dialog.selected_model, 1);

    // The official list moves gpt-4-turbo to the last position.
    assert!(dialog.replace_models(
        "openai",
        vec![model("gpt-4o"), model("o3"), model("gpt-4-turbo")],
    ));

    assert_eq!(
        ids(dialog.current_models_ref()),
        vec!["gpt-4o", "o3", "gpt-4-turbo"]
    );
    assert_eq!(
        dialog.selected(),
        Some(("openai".to_string(), "gpt-4-turbo".to_string())),
        "选中项要跟着 id 走，不能被下标带偏"
    );
}

#[test]
fn replace_models_clamps_selection_when_id_disappears() {
    let mut dialog =
        ProviderDialog::new_with_selection(providers(), Some("deepseek"), Some("deepseek-v4-pro"));
    assert_eq!(dialog.selected_model, 1);

    assert!(dialog.replace_models("deepseek", vec![model("deepseek-chat")]));

    assert_eq!(dialog.selected_model, 0);
    assert_eq!(
        dialog.selected(),
        Some(("deepseek".to_string(), "deepseek-chat".to_string()))
    );
}

/// By the time an outcome lands the user may have switched to another
/// provider: the selection on the other provider must not be recalculated.
#[test]
fn replace_models_leaves_selection_alone_for_non_selected_provider() {
    let mut dialog =
        ProviderDialog::new_with_selection(providers(), Some("openai"), Some("gpt-4-turbo"));
    dialog.move_down();
    let selected_after_move = dialog.selected_model;

    assert!(dialog.replace_models("openai", vec![model("a"), model("b")]));

    assert_eq!(dialog.selected_provider, 1);
    assert_eq!(
        dialog.selected_model, selected_after_move,
        "被替换的不是当前高亮 provider 时，不动当前选中下标"
    );
    assert_eq!(ids(&dialog.providers[0].models), vec!["a", "b"]);
}

#[test]
fn replace_models_rejects_empty_list_and_unknown_provider() {
    let mut dialog =
        ProviderDialog::new_with_selection(providers(), Some("openai"), Some("gpt-4o"));

    assert!(
        !dialog.replace_models("openai", Vec::new()),
        "空列表由调用方按失败处理，这里必须拒绝且不改动状态"
    );
    assert!(!dialog.replace_models("no-such-provider", vec![model("x")]));

    assert_eq!(
        ids(dialog.current_models_ref()),
        vec!["gpt-4o", "gpt-4-turbo"],
        "两次拒绝之后列表必须保持原样"
    );
    assert_eq!(dialog.selected_model, 0);
}
