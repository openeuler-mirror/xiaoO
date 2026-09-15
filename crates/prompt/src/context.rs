use agent_contracts::PromptBuildInput;
use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
use agent_types::ChatMessage;

#[derive(Debug, Clone)]
pub struct InstructionContext {
    pub source: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct CompressedHistory {
    pub compressed_messages: Vec<ChatMessage>,
    pub recent_tail: Vec<ChatMessage>,
    pub summary: Option<String>,
}

impl CompressedHistory {
    pub fn from_messages(messages: Vec<ChatMessage>) -> Self {
        Self {
            compressed_messages: messages,
            recent_tail: Vec::new(),
            summary: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PromptContext {
    pub environment: EnvironmentInfo,
    pub instructions: Vec<InstructionContext>,
    pub memory_snippets: Vec<MemorySnippet>,
    pub skill_snippets: Vec<SkillSummary>,
    pub history: CompressedHistory,
}

pub fn collect_prompt_context(input: &PromptBuildInput) -> PromptContext {
    let (instructions, memory_snippets) = split_instruction_snippets(&input.memory_snippets);

    PromptContext {
        environment: input.environment.clone(),
        instructions,
        memory_snippets,
        skill_snippets: input.skill_summaries.clone(),
        history: CompressedHistory::from_messages(input.messages.clone()),
    }
}

fn split_instruction_snippets(
    snippets: &[MemorySnippet],
) -> (Vec<InstructionContext>, Vec<MemorySnippet>) {
    let mut instructions = Vec::new();
    let mut memory_snippets = Vec::new();

    for snippet in snippets {
        if let Some(source) = snippet.source.strip_prefix("instruction:") {
            instructions.push(InstructionContext {
                source: source.trim().to_string(),
                content: snippet.content.clone(),
            });
        } else if snippet.source.trim() == "instruction" {
            instructions.push(InstructionContext {
                source: "memory".to_string(),
                content: snippet.content.clone(),
            });
        } else {
            memory_snippets.push(snippet.clone());
        }
    }

    (instructions, memory_snippets)
}

#[cfg(test)]
#[path = "../../../tests/unit/prompt/context_test.rs"]
mod tests;
