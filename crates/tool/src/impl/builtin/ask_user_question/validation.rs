//! Input validation for AskUserQuestionTool.
//!
//! Validates AskUserQuestionInput before processing to ensure:
//! - questions 列表包含 1–4 个条目
//! - 每个问题的 prompt 非空
//! - Choice 类型问题至少有 2 个选项，且每个选项非空
//! - questions 的 prompt 在列表内唯一（不重复）
//! - Choice 类型问题的选项在该问题内唯一

use super::input::{AskUserQuestionInput, QuestionItem};

/// Validation error codes.
pub mod error_code {
    /// questions 列表为空（error_code = 1）
    pub const QUESTIONS_EMPTY: u32 = 1;
    /// questions 列表超过 4 个（error_code = 2）
    pub const QUESTIONS_TOO_MANY: u32 = 2;
    /// 某个问题的 prompt 为空字符串（error_code = 3）
    pub const PROMPT_EMPTY: u32 = 3;
    /// Choice 选项数量不足 2 个（error_code = 4）
    pub const CHOICE_TOO_FEW_OPTIONS: u32 = 4;
    /// Choice 某个选项为空字符串（error_code = 5）
    pub const CHOICE_OPTION_EMPTY: u32 = 5;
    /// questions 中存在重复的 prompt（error_code = 6）
    pub const DUPLICATE_PROMPT: u32 = 6;
    /// 同一 Choice 问题内存在重复选项（error_code = 7）
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
    // 1. 数量下限
    if input.questions.is_empty() {
        return ValidationResult::error(
            "questions 不能为空，至少需要 1 个问题",
            error_code::QUESTIONS_EMPTY,
        );
    }

    // 2. 数量上限
    if input.questions.len() > 4 {
        return ValidationResult::error(
            format!(
                "questions 最多包含 4 个问题，当前有 {} 个",
                input.questions.len()
            ),
            error_code::QUESTIONS_TOO_MANY,
        );
    }

    // 3. 逐条校验每个问题
    for (idx, question) in input.questions.iter().enumerate() {
        let pos = idx + 1; // 1-based index for user-facing messages

        match question {
            QuestionItem::Confirm { prompt } | QuestionItem::TextInput { prompt } => {
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
                // prompt 非空
                if prompt.trim().is_empty() {
                    return ValidationResult::error(
                        format!("第 {} 个问题的 prompt 不能为空", pos),
                        error_code::PROMPT_EMPTY,
                    );
                }

                // options 数量下限
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

                // 每个 option 非空
                for (opt_idx, opt) in options.iter().enumerate() {
                    if opt.trim().is_empty() {
                        return ValidationResult::error(
                            format!("第 {} 个问题的第 {} 个选项不能为空", pos, opt_idx + 1),
                            error_code::CHOICE_OPTION_EMPTY,
                        );
                    }
                }

                // 选项唯一性（大小写敏感）
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

    // 4. questions prompt 唯一性
    let prompts: Vec<&str> = input
        .questions
        .iter()
        .map(|q| match q {
            QuestionItem::Confirm { prompt }
            | QuestionItem::TextInput { prompt }
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
