use serde::{Deserialize, Serialize};

/// Answer to a confirm question, serialized as "yes" / "no" / "unanswered".
///
/// The previous shape (`kind: "confirmed"` + `allowed: bool`) read like a
/// permission verdict: for prompts that describe two courses of action
/// (选「是」→X / 选「否」→Y) some models mapped `allowed: false` to the
/// opposite branch of what the user actually chose. A literal "yes"/"no"
/// answer carries no such ambiguity.
///
/// `Unanswered` means no answer was received (cancelled / closed /
/// timeout); it is not a denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfirmAnswer {
    #[serde(rename = "yes")]
    Yes,
    #[serde(rename = "no")]
    No,
    #[serde(rename = "unanswered")]
    Unanswered,
}

impl From<bool> for ConfirmAnswer {
    fn from(allowed: bool) -> Self {
        if allowed {
            Self::Yes
        } else {
            Self::No
        }
    }
}

/// Three variants corresponding to InteractionResponse, carrying original prompt for AI reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnswerItem {
    /// Response to Confirm request: `answer` mirrors the question's own
    /// yes/no proposition ("yes" = 是, "no" = 否, "unanswered" = 用户未作答).
    Confirm {
        prompt: String,
        answer: ConfirmAnswer,
    },
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

#[cfg(test)]
#[path = "../../../../../../tests/unit/tool/impl/builtin/ask_user_question/output_test.rs"]
mod tests;
