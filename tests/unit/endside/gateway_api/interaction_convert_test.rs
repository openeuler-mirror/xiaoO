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

/// Decision table for the Choice flag/hint mapping: the flag maps
/// faithfully for every source (permission backends only accept their
/// exact option strings), while the type-answer hint is exclusively for
/// Tool-sourced requests with custom input allowed.
#[test]
fn build_prompt_request_choice_flag_and_hint_follow_the_source() {
    for (source, allow, expect_hint) in [
        // Tool + custom input → type-answer hint.
        (Some(ask_user_source()), true, true),
        // A Tool-sourced `false` (none exists today) must be honored, not
        // silently widened: the always-allow guarantee lives in the executor.
        (Some(ask_user_source()), false, false),
        // Non-tool sources keep the flag semantics but never the hint.
        (Some(hooker_source()), true, false),
        (Some(hooker_source()), false, false),
        (Some(InteractionSource::System), true, false),
        (Some(InteractionSource::System), false, false),
        (None, true, false),
        (None, false, false),
    ] {
        let request = choice_request(allow, source);
        let prompt = build_prompt_request(&request);
        assert_eq!(prompt.allow_custom_input, allow);
        assert_eq!(
            prompt.custom_input_hint.as_deref(),
            expect_hint.then_some(TYPE_ANSWER_HINT)
        );
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

/// Decision table for `map_response` on Choice requests: a typed answer
/// submitted from the supplement box wins; a blank supplement falls back
/// to the selected option; residue typed in the box but submitted from
/// the list must not override the selection.
#[test]
fn choice_response_maps_single_result_by_submission_origin() {
    // (choice_id, supplement, submitted_from_supplement, expected value)
    for (choice_id, supplement, from_supplement, expected) in [
        ("a", Some("my own answer"), true, "my own answer"),
        ("b", None, true, "b"),
        ("a", Some("stale residue"), false, "a"),
    ] {
        let request = choice_request(false, Some(ask_user_source()));
        let result = single_result(choice_id, supplement.map(str::to_string), from_supplement);
        match map_response(&request, result) {
            Some(InteractionResponse::Choice { value }) => {
                assert_eq!(value.as_deref(), Some(expected));
            }
            other => panic!("expected Choice response, got {:?}", other),
        }
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
