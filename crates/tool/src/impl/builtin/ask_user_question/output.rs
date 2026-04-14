use serde::{Deserialize, Serialize};

/// 对应 InteractionResponse 的三种变体，同时携带原始 prompt 便于 AI 对应问答。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerItem {
    /// 对 Confirm 请求的回答
    Confirmed { prompt: String, allowed: bool },
    /// 对 TextInput 请求的回答
    Text {
        prompt: String,
        value: Option<String>,
    },
    /// 对 Choice 请求的回答
    Choice {
        prompt: String,
        value: Option<String>,
    },
}

/// AskUserQuestion 工具的输出结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestionOutput {
    /// 与输入问题一一对应的回答列表。
    pub answers: Vec<AnswerItem>,
}
