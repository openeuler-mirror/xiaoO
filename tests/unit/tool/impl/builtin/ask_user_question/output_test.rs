use super::*;

#[test]
fn confirm_answer_serializes_explicit_yes_no() {
    let yes = AnswerItem::Confirm {
        prompt: "是否恢复该历史任务？".to_string(),
        answer: ConfirmAnswer::Yes,
    };
    let no = AnswerItem::Confirm {
        prompt: "是否恢复该历史任务？".to_string(),
        answer: ConfirmAnswer::No,
    };

    assert_eq!(
        serde_json::to_value(&yes).unwrap(),
        serde_json::json!({
            "kind": "confirm",
            "prompt": "是否恢复该历史任务？",
            "answer": "yes"
        })
    );
    assert_eq!(
        serde_json::to_value(&no).unwrap(),
        serde_json::json!({
            "kind": "confirm",
            "prompt": "是否恢复该历史任务？",
            "answer": "no"
        })
    );
}

#[test]
fn confirm_answer_deserializes_yes_no() {
    let answer: AnswerItem = serde_json::from_value(serde_json::json!({
        "kind": "confirm",
        "prompt": "继续吗？",
        "answer": "no"
    }))
    .unwrap();
    assert_eq!(
        answer,
        AnswerItem::Confirm {
            prompt: "继续吗？".to_string(),
            answer: ConfirmAnswer::No
        }
    );
}

#[test]
fn confirm_answer_from_bool() {
    assert_eq!(ConfirmAnswer::from(true), ConfirmAnswer::Yes);
    assert_eq!(ConfirmAnswer::from(false), ConfirmAnswer::No);
}

#[test]
fn confirm_answer_unanswered_round_trips() {
    let item = AnswerItem::Confirm {
        prompt: "是否恢复该历史任务？".to_string(),
        answer: ConfirmAnswer::Unanswered,
    };

    let encoded = serde_json::to_value(&item).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({
            "kind": "confirm",
            "prompt": "是否恢复该历史任务？",
            "answer": "unanswered"
        })
    );

    let decoded: AnswerItem = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, item);
}

#[test]
fn confirm_answer_rejects_unknown_values() {
    let result: Result<AnswerItem, _> = serde_json::from_value(serde_json::json!({
        "kind": "confirm",
        "prompt": "继续吗？",
        "answer": "maybe"
    }));
    assert!(result.is_err());
}

#[test]
fn output_serializes_answers_in_order() {
    let output = AskUserQuestionOutput {
        answers: vec![
            AnswerItem::Confirm {
                prompt: "确认？".to_string(),
                answer: ConfirmAnswer::No,
            },
            AnswerItem::Text {
                prompt: "名字".to_string(),
                value: Some("xiaoO".to_string()),
                display_value: None,
            },
            AnswerItem::Choice {
                prompt: "选择".to_string(),
                value: Some("全新任务".to_string()),
            },
        ],
    };
    assert_eq!(
        serde_json::to_value(&output).unwrap(),
        serde_json::json!({
            "answers": [
                { "kind": "confirm", "prompt": "确认？", "answer": "no" },
                { "kind": "text", "prompt": "名字", "value": "xiaoO" },
                { "kind": "choice", "prompt": "选择", "value": "全新任务" }
            ]
        })
    );
}
