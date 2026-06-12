use serde::{Deserialize, Serialize};

/// Three variants corresponding to InteractionResponse, carrying original prompt for AI reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerItem {
    /// Response to Confirm request
    Confirmed { prompt: String, allowed: bool },
    /// Response to TextInput request
    Text {
        prompt: String,
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_value: Option<String>,
    },
    /// Response to Choice request
    Choice {
        prompt: String,
        value: Option<String>,
    },
}

/// Output structure for AskUserQuestion tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestionOutput {
    /// List of answers corresponding one-to-one with input questions.
    pub answers: Vec<AnswerItem>,
}
