use agent_contracts::TokenEstimator;
use agent_types::compression::ContextAnalysis;
use agent_types::ChatMessage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextSection {
    pub label: String,
    pub message_count: usize,
    pub token_estimate: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextBreakdown {
    pub total_tokens: usize,
    pub head_tokens: usize,
    pub summary_tokens: usize,
    pub tail_tokens: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContextEnvelope {
    pub sections: Vec<ContextSection>,
    pub breakdown: ContextBreakdown,
    pub analysis: ContextAnalysis,
}

impl ContextEnvelope {
    pub fn build(
        messages: &[ChatMessage],
        summary: Option<&str>,
        estimator: &dyn TokenEstimator,
        analysis: ContextAnalysis,
        preserve_tail_messages: usize,
    ) -> Self {
        let tail_start = messages.len().saturating_sub(preserve_tail_messages);
        let head = &messages[..tail_start];
        let tail = &messages[tail_start..];
        let head_tokens = estimator.estimate_messages_tokens(head);
        let tail_tokens = estimator.estimate_messages_tokens(tail);
        let summary_tokens = summary
            .map(|summary| estimator.estimate_text_tokens(summary))
            .unwrap_or(0);
        let total_tokens = head_tokens + tail_tokens + summary_tokens;

        let mut sections = vec![
            ContextSection {
                label: "head".to_string(),
                message_count: head.len(),
                token_estimate: head_tokens,
            },
            ContextSection {
                label: "tail".to_string(),
                message_count: tail.len(),
                token_estimate: tail_tokens,
            },
        ];

        if summary.is_some() {
            sections.insert(
                1,
                ContextSection {
                    label: "summary".to_string(),
                    message_count: 1,
                    token_estimate: summary_tokens,
                },
            );
        }

        Self {
            sections,
            breakdown: ContextBreakdown {
                total_tokens,
                head_tokens,
                summary_tokens,
                tail_tokens,
            },
            analysis,
        }
    }
}
