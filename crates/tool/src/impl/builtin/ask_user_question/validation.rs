//! Input validation for AskUserQuestionTool.
//!
//! Validates AskUserQuestionInput before processing to ensure:
//! - questions list contains 1–4 entries
//! - each question's prompt is non-empty
//! - Choice type questions have at least 2 options, each non-empty
//! - questions' prompts are unique within the list (no duplicates)
//! - Choice type questions' options are unique within that question

use super::input::{AskUserQuestionInput, QuestionItem};

/// Validation error codes.
pub mod error_code {
    /// questions list is empty (error_code = 1)
    pub const QUESTIONS_EMPTY: u32 = 1;
    /// questions list exceeds 4 items (error_code = 2)
    pub const QUESTIONS_TOO_MANY: u32 = 2;
    /// a question's prompt is empty string (error_code = 3)
    pub const PROMPT_EMPTY: u32 = 3;
    /// Choice has fewer than 2 options (error_code = 4)
    pub const CHOICE_TOO_FEW_OPTIONS: u32 = 4;
    /// a Choice option is empty string (error_code = 5)
    pub const CHOICE_OPTION_EMPTY: u32 = 5;
    /// duplicate prompts in questions (error_code = 6)
    pub const DUPLICATE_PROMPT: u32 = 6;
    /// duplicate options within a Choice question (error_code = 7)
    pub const DUPLICATE_CHOICE_OPTION: u32 = 7;
}

/// Result of input validation.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether validation passed.
    pub result: bool,
    /// Error message if validation failed.
    pub message: Option<String>,
    /// Error code if validation failed.
    pub error_code: Option<u32>,
}

impl ValidationResult {
    pub fn ok() -> Self {
        Self {
            result: true,
            message: None,
            error_code: None,
        }
    }

    pub fn error(message: impl Into<String>, error_code: u32) -> Self {
        Self {
            result: false,
            message: Some(message.into()),
            error_code: Some(error_code),
        }
    }
}

/// Validates AskUserQuestionInput.
pub fn validate_input(input: &AskUserQuestionInput) -> ValidationResult {
    // 1. Minimum count
    if input.questions.is_empty() {
        return ValidationResult::error(
            "questions 不能为空，至少需要 1 个问题",
            error_code::QUESTIONS_EMPTY,
        );
    }

    // 2. Maximum count
    if input.questions.len() > 4 {
        return ValidationResult::error(
            format!(
                "questions 最多包含 4 个问题，当前有 {} 个",
                input.questions.len()
            ),
            error_code::QUESTIONS_TOO_MANY,
        );
    }

    // 3. Validate each question
    for (idx, question) in input.questions.iter().enumerate() {
        let pos = idx + 1; // 1-based index for user-facing messages

        match question {
            QuestionItem::Confirm { prompt } | QuestionItem::TextInput { prompt, .. } => {
                if prompt.trim().is_empty() {
                    return ValidationResult::error(
                        format!("第 {} 个问题的 prompt 不能为空", pos),
                        error_code::PROMPT_EMPTY,
                    );
                }
            }
            QuestionItem::Choice {
                prompt, options, ..
            } => {
                // prompt must be non-empty
                if prompt.trim().is_empty() {
                    return ValidationResult::error(
                        format!("第 {} 个问题的 prompt 不能为空", pos),
                        error_code::PROMPT_EMPTY,
                    );
                }

                // options minimum count
                if options.len() < 2 {
                    return ValidationResult::error(
                        format!(
                            "第 {} 个问题（Choice 类型）至少需要 2 个选项，当前有 {} 个",
                            pos,
                            options.len()
                        ),
                        error_code::CHOICE_TOO_FEW_OPTIONS,
                    );
                }

                // each option must be non-empty
                for (opt_idx, opt) in options.iter().enumerate() {
                    if opt.trim().is_empty() {
                        return ValidationResult::error(
                            format!("第 {} 个问题的第 {} 个选项不能为空", pos, opt_idx + 1),
                            error_code::CHOICE_OPTION_EMPTY,
                        );
                    }
                }

                // option uniqueness (case-sensitive)
                let unique_options: std::collections::HashSet<&str> =
                    options.iter().map(|o| o.as_str()).collect();
                if unique_options.len() != options.len() {
                    return ValidationResult::error(
                        format!("第 {} 个问题（Choice 类型）存在重复的选项", pos),
                        error_code::DUPLICATE_CHOICE_OPTION,
                    );
                }
            }
        }
    }

    // 4. questions prompt uniqueness
    let prompts: Vec<&str> = input
        .questions
        .iter()
        .map(|q| match q {
            QuestionItem::Confirm { prompt }
            | QuestionItem::TextInput { prompt, .. }
            | QuestionItem::Choice { prompt, .. } => prompt.as_str(),
        })
        .collect();

    let unique_prompts: std::collections::HashSet<&str> = prompts.iter().copied().collect();
    if unique_prompts.len() != prompts.len() {
        return ValidationResult::error(
            "questions 中存在重复的 prompt，每个问题的文本必须唯一",
            error_code::DUPLICATE_PROMPT,
        );
    }

    ValidationResult::ok()
}

#[cfg(test)]
mod tests {
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
    fn test_empty_questions() {
        let input = AskUserQuestionInput { questions: vec![] };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::QUESTIONS_EMPTY));
    }

    #[test]
    fn test_too_many_questions() {
        let input = AskUserQuestionInput {
            questions: (0..5).map(|i| confirm(&format!("q{}", i))).collect(),
        };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::QUESTIONS_TOO_MANY));
    }

    #[test]
    fn test_empty_prompt() {
        let input = AskUserQuestionInput {
            questions: vec![confirm("  ")],
        };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::PROMPT_EMPTY));
    }

    #[test]
    fn test_choice_too_few_options() {
        let input = AskUserQuestionInput {
            questions: vec![choice("pick one?", &["only one"])],
        };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::CHOICE_TOO_FEW_OPTIONS));
    }

    #[test]
    fn test_choice_empty_option() {
        let input = AskUserQuestionInput {
            questions: vec![choice("pick?", &["a", ""])],
        };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::CHOICE_OPTION_EMPTY));
    }

    #[test]
    fn test_duplicate_prompts() {
        let input = AskUserQuestionInput {
            questions: vec![confirm("same?"), text_input("same?")],
        };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::DUPLICATE_PROMPT));
    }

    #[test]
    fn test_duplicate_choice_options() {
        let input = AskUserQuestionInput {
            questions: vec![choice("pick?", &["a", "a"])],
        };
        let r = validate_input(&input);
        assert!(!r.result);
        assert_eq!(r.error_code, Some(error_code::DUPLICATE_CHOICE_OPTION));
    }

    #[test]
    fn test_valid_input() {
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
}
