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

#[test]
fn empty_questions_are_rejected() {
    let input = AskUserQuestionInput { questions: vec![] };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::QUESTIONS_EMPTY));
}

#[test]
fn more_than_four_questions_are_rejected() {
    let input = AskUserQuestionInput {
        questions: (0..5).map(|i| confirm(&format!("q{}", i))).collect(),
    };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::QUESTIONS_TOO_MANY));
}

#[test]
fn blank_prompt_is_rejected() {
    let input = AskUserQuestionInput {
        questions: vec![confirm("  ")],
    };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::PROMPT_EMPTY));
}

#[test]
fn choice_with_fewer_than_two_options_is_rejected() {
    let input = AskUserQuestionInput {
        questions: vec![choice("pick one?", &["only one"])],
    };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::CHOICE_TOO_FEW_OPTIONS));
}

#[test]
fn choice_with_blank_option_is_rejected() {
    let input = AskUserQuestionInput {
        questions: vec![choice("pick?", &["a", ""])],
    };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::CHOICE_OPTION_EMPTY));
}

#[test]
fn duplicate_prompts_are_rejected() {
    let input = AskUserQuestionInput {
        questions: vec![confirm("same?"), text_input("same?")],
    };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::DUPLICATE_PROMPT));
}

#[test]
fn duplicate_choice_options_are_rejected() {
    let input = AskUserQuestionInput {
        questions: vec![choice("pick?", &["a", "a"])],
    };
    let r = validate_input(&input);
    assert!(!r.result);
    assert_eq!(r.error_code, Some(error_code::DUPLICATE_CHOICE_OPTION));
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
