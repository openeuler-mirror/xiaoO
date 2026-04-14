use serde::{Deserialize, Serialize};

/// 对应 InteractionRequest 的三种变体，用 serde tag 区分。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionItem {
    /// 确认类问题（是/否）
    Confirm { prompt: String },
    /// 文本输入类问题
    TextInput { prompt: String },
    /// 单选/多选类问题
    Choice {
        prompt: String,
        options: Vec<String>,
        #[serde(default)]
        allow_custom_input: bool,
    },
}

/// AskUserQuestion 工具的输入结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    /// 要向用户提出的问题列表（1–4 个）。
    pub questions: Vec<QuestionItem>,
}
