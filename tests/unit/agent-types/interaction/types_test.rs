use super::*;

fn request(kind: &str) -> InteractionRequest {
    match kind {
        "confirm" => InteractionRequest::Confirm {
            prompt: "继续吗？".to_string(),
            source: None,
        },
        "text" => InteractionRequest::TextInput {
            prompt: "任务名称：".to_string(),
            source: None,
            is_secret: false,
        },
        _ => InteractionRequest::Choice {
            prompt: "选择：".to_string(),
            options: vec!["A 选项".to_string(), "B 选项".to_string()],
            allow_custom_input: false,
            source: None,
        },
    }
}

#[test]
fn unanswered_confirm_is_distinct_from_explicit_denial() {
    let response = InteractionResponse::unanswered(&request("confirm"));

    // An unanswered confirm must not look like the user said no.
    assert!(matches!(response, InteractionResponse::Unanswered));
    assert_eq!(
        serde_json::to_value(&response).unwrap(),
        serde_json::json!({ "kind": "unanswered" })
    );
}

#[test]
fn unanswered_text_and_choice_carry_no_value() {
    assert!(matches!(
        InteractionResponse::unanswered(&request("text")),
        InteractionResponse::Text { value: None, .. }
    ));
    assert!(matches!(
        InteractionResponse::unanswered(&request("choice")),
        InteractionResponse::Choice { value: None }
    ));
}

#[test]
fn unanswered_round_trips_through_serde() {
    let response = InteractionResponse::unanswered(&request("confirm"));
    let decoded: InteractionResponse =
        serde_json::from_value(serde_json::to_value(&response).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(&decoded).unwrap(),
        serde_json::json!({ "kind": "unanswered" })
    );
}
