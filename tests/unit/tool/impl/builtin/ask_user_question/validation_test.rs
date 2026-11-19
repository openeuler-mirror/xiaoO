use super::*;
use crate::r#impl::builtin::ask_user_question::input::QuestionItem;

fn confirm(prompt: &str) -> QuestionItem {
    QuestionItem::Confirm {
        prompt: prompt.to_string(),
    }
}

fn text_input(prompt: &str) -> QuestionItem {
    QuestionItem::TextInput {
        prompt: prompt.to_string(),
        is_secret: false,
    }
}

fn choice(prompt: &str, options: &[&str]) -> QuestionItem {
    QuestionItem::Choice {
        prompt: prompt.to_string(),
        options: options.iter().map(|s| s.to_string()).collect(),
        allow_custom_input: false,
    }
}

/// Each invalid question shape is rejected with its specific error code:
/// (questions, expected error_code).
#[test]
fn invalid_question_sets_are_rejected_with_specific_error_codes() {
    let cases: Vec<(Vec<QuestionItem>, u32)> = vec![
        (vec![], error_code::QUESTIONS_EMPTY),
        // (0..5) confirms: more than four questions.
        (
            (0..5).map(|i| confirm(&format!("q{i}"))).collect(),
            error_code::QUESTIONS_TOO_MANY,
        ),
        (vec![confirm("  ")], error_code::PROMPT_EMPTY),
        (
            vec![choice("pick one?", &["only one"])],
            error_code::CHOICE_TOO_FEW_OPTIONS,
        ),
        (
            vec![choice("pick?", &["a", ""])],
            error_code::CHOICE_OPTION_EMPTY,
        ),
        (
            vec![confirm("same?"), text_input("same?")],
            error_code::DUPLICATE_PROMPT,
        ),
        (
            vec![choice("pick?", &["a", "a"])],
            error_code::DUPLICATE_CHOICE_OPTION,
        ),
    ];
    for (questions, expected) in cases {
        let r = validate_input(&AskUserQuestionInput {
            questions: questions.clone(),
        });
        assert!(!r.result, "questions: {questions:?}");
        assert_eq!(r.error_code, Some(expected), "questions: {questions:?}");
    }
}

#[test]
fn mixed_question_types_pass_validation() {
    let input = AskUserQuestionInput {
        questions: vec![
            confirm("确认继续？"),
            text_input("输入名称："),
            choice("选择方案：", &["方案 A", "方案 B", "方案 C"]),
        ],
    };
    let r = validate_input(&input);
    assert!(r.result);
}
