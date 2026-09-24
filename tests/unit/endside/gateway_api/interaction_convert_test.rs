use xiaoo_api::interaction::{InteractionRequest, InteractionResponse, InteractionSource};

use super::{build_prompt_request, map_response};
use crate::interaction_prompt::{PromptResolution, UserPromptResult, TYPE_ANSWER_HINT};

fn choice_request(
    allow_custom_input: bool,
    source: Option<InteractionSource>,
) -> InteractionRequest {
    InteractionRequest::Choice {
        prompt: "Pick one".to_string(),
        options: vec!["a".to_string(), "b".to_string()],
        allow_custom_input,
        source,
    }
}

fn ask_user_source() -> InteractionSource {
    InteractionSource::Tool {
        tool_name: "ask_user_question".to_string(),
    }
}

fn hooker_source() -> InteractionSource {
    InteractionSource::Hooker {
        hooker_name: "plugin".to_string(),
        hook_point: "chat".to_string(),
    }
}

#[test]
fn choice_flag_is_mapped_faithfully_for_tool_source_too() {
    // The always-allow guarantee lives in the executor; a Tool-sourced `false`
    // (none exists today) must be honored, not silently widened.
    let request = choice_request(false, Some(ask_user_source()));
    let prompt = build_prompt_request(&request);
    assert!(!prompt.allow_custom_input);
    assert!(prompt.custom_input_hint.is_none());
}

#[test]
fn tool_choice_with_custom_input_uses_type_answer_hint() {
    let request = choice_request(true, Some(ask_user_source()));
    let prompt = build_prompt_request(&request);
    assert!(prompt.allow_custom_input);
    assert_eq!(prompt.custom_input_hint.as_deref(), Some(TYPE_ANSWER_HINT));
}

#[test]
fn permission_style_choice_keeps_custom_input_disabled() {
    // Permission backends only accept their exact option strings.
    let request = choice_request(false, None);
    let prompt = build_prompt_request(&request);
    assert!(!prompt.allow_custom_input);
    assert!(prompt.custom_input_hint.is_none());
}

#[test]
fn non_tool_sources_keep_flag_semantics_and_default_hint() {
    for source in [Some(hooker_source()), Some(InteractionSource::System), None] {
        let enabled = choice_request(true, source.clone());
        let prompt = build_prompt_request(&enabled);
        assert!(prompt.allow_custom_input);
        assert!(prompt.custom_input_hint.is_none());

        let disabled = choice_request(false, source);
        let prompt = build_prompt_request(&disabled);
        assert!(!prompt.allow_custom_input);
        assert!(prompt.custom_input_hint.is_none());
    }
}

#[test]
fn confirm_and_text_input_keep_default_hint() {
    let confirm = InteractionRequest::Confirm {
        prompt: "Proceed?".to_string(),
        source: Some(ask_user_source()),
    };
    let prompt = build_prompt_request(&confirm);
    assert!(!prompt.allow_custom_input);
    assert!(prompt.custom_input_hint.is_none());

    let text_input = InteractionRequest::TextInput {
        prompt: "Your name?".to_string(),
        source: Some(ask_user_source()),
        is_secret: false,
    };
    let prompt = build_prompt_request(&text_input);
    assert!(prompt.allow_custom_input);
    assert!(prompt.custom_input_hint.is_none());
}

fn single_result(
    choice_id: &str,
    supplement: Option<String>,
    submitted_from_supplement: bool,
) -> UserPromptResult {
    UserPromptResult {
        request_id: "req-1".to_string(),
        resolution: PromptResolution::Single {
            choice_id: choice_id.to_string(),
            supplement,
            submitted_from_supplement,
        },
    }
}

#[test]
fn choice_response_prefers_typed_answer_over_selected_option() {
    let request = choice_request(false, None);
    let result = single_result("a", Some("my own answer".to_string()), true);
    match map_response(&request, result) {
        Some(InteractionResponse::Choice { value }) => {
            assert_eq!(value.as_deref(), Some("my own answer"));
        }
        other => panic!("expected Choice response, got {:?}", other),
    }
}

#[test]
fn choice_response_falls_back_to_selected_option_when_answer_blank() {
    let request = choice_request(false, None);
    let result = single_result("b", None, true);
    match map_response(&request, result) {
        Some(InteractionResponse::Choice { value }) => {
            assert_eq!(value.as_deref(), Some("b"));
        }
        other => panic!("expected Choice response, got {:?}", other),
    }
}

#[test]
fn choice_response_ignores_supplement_residue_when_list_selection_submitted() {
    let request = choice_request(false, Some(ask_user_source()));
    // Typed in the box, then submitted from the list: residue must not win.
    let result = single_result("a", Some("stale residue".to_string()), false);
    match map_response(&request, result) {
        Some(InteractionResponse::Choice { value }) => {
            assert_eq!(value.as_deref(), Some("a"));
        }
        other => panic!("expected Choice response, got {:?}", other),
    }
}

#[test]
fn text_input_keeps_typed_answer_when_submitted_from_list() {
    let request = InteractionRequest::TextInput {
        prompt: "Your name?".to_string(),
        source: None,
        is_secret: false,
    };
    // The supplement box carries the answer itself.
    let result = single_result("submit", Some("Ada".to_string()), false);
    match map_response(&request, result) {
        Some(InteractionResponse::Text {
            value,
            display_value,
        }) => {
            assert_eq!(value.as_deref(), Some("Ada"));
            assert!(display_value.is_none());
        }
        other => panic!("expected Text response, got {:?}", other),
    }
}

#[test]
fn cancelled_resolution_maps_to_no_response() {
    let request = choice_request(false, None);
    let result = UserPromptResult {
        request_id: "req-1".to_string(),
        resolution: PromptResolution::Cancelled,
    };
    assert!(map_response(&request, result).is_none());
}
