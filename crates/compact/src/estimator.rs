use agent_contracts::TokenEstimator;
use agent_types::{ChatMessage, ContentBlock};
use serde::{Deserialize, Serialize};

use crate::{CompactError, CompactResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoughTokenEstimatorConfig {
    pub chars_per_token: usize,
    pub message_overhead_tokens: usize,
    pub tool_use_overhead_tokens: usize,
    pub tool_result_overhead_tokens: usize,
    pub image_block_overhead_tokens: usize,
    pub document_block_overhead_tokens: usize,
}

pub struct RoughTokenEstimator {
    config: RoughTokenEstimatorConfig,
}

impl RoughTokenEstimator {
    pub fn try_new(config: RoughTokenEstimatorConfig) -> CompactResult<Self> {
        if config.chars_per_token == 0 {
            return Err(CompactError::InvalidConfiguration {
                message: "chars_per_token must be greater than zero".to_string(),
            });
        }

        Ok(Self { config })
    }

    pub fn config(&self) -> &RoughTokenEstimatorConfig {
        &self.config
    }
}

impl TokenEstimator for RoughTokenEstimator {
    fn estimate_message_tokens(&self, message: &ChatMessage) -> usize {
        self.config.message_overhead_tokens
            + message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => self.estimate_text_tokens(text),
                    ContentBlock::ToolUse {
                        tool_name, input, ..
                    } => {
                        self.config.tool_use_overhead_tokens
                            + self.estimate_text_tokens(tool_name)
                            + self.estimate_text_tokens(&input.to_string())
                    }
                    ContentBlock::ToolResult {
                        tool_name, output, ..
                    } => {
                        self.config.tool_result_overhead_tokens
                            + self.estimate_text_tokens(tool_name)
                            + self.estimate_text_tokens(output)
                    }
                    ContentBlock::Image { description } => {
                        self.config.image_block_overhead_tokens
                            + self.estimate_text_tokens(description)
                    }
                    ContentBlock::Document { description } => {
                        self.config.document_block_overhead_tokens
                            + self.estimate_text_tokens(description)
                    }
                })
                .sum::<usize>()
    }

    fn estimate_messages_tokens(&self, messages: &[ChatMessage]) -> usize {
        messages
            .iter()
            .map(|message| self.estimate_message_tokens(message))
            .sum()
    }

    fn estimate_text_tokens(&self, text: &str) -> usize {
        let characters = text.chars().count();
        if characters == 0 {
            return 0;
        }

        characters.div_ceil(self.config.chars_per_token)
    }
}
