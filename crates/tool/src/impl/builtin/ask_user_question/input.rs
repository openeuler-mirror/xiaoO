use serde::{Deserialize, Serialize};

/// Three variants corresponding to InteractionRequest, differentiated by serde tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionItem {
    /// Confirmation question (yes/no)
    Confirm { prompt: String },
    /// Text input question
    TextInput {
        prompt: String,
        #[serde(default)]
        is_secret: bool,
    },
    /// Single/multi-choice question
    Choice {
        prompt: String,
        options: Vec<String>,
        #[serde(default)]
        allow_custom_input: bool,
    },
}

/// Input structure for AskUserQuestion tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    /// List of questions to ask the user (1–4 items).
    pub questions: Vec<QuestionItem>,
}
