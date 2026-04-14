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
mod tests {
    use super::*;
    use agent_llm::ChatMessageExt;

    #[test]
    #[ignore]
    fn collect_prompt_context_lifts_instruction_snippets_out_of_memory_section() {
        let input = PromptBuildInput {
            system_prompt: "system".to_string(),
            messages: vec![ChatMessage::user("hello")],
            visible_tools: Vec::new(),
            skill_summaries: Vec::new(),
            memory_snippets: vec![
                MemorySnippet {
                    source: "instruction:policy".to_string(),
                    content: "be precise".to_string(),
                    relevance_score: 1.0,
                },
                MemorySnippet {
                    source: "fact:repo".to_string(),
                    content: "XiaoO".to_string(),
                    relevance_score: 2.0,
                },
            ],
            environment: EnvironmentInfo {
                model: String::new(),
                cwd: String::new(),
                workspace_root: None,
                date: String::new(),
                agent_id: String::new(),
            },
            feature_flags: agent_types::context::features::FeatureFlags::default(),
            turn_count: 1,
            budget: agent_types::TokenBudgetConfig {
                total_budget: 128,
                reserved_for_output: 16,
                reserved_for_system: 16,
                hard_limit_ratio: 0.8,
            },
        };

        let context = collect_prompt_context(&input);

        assert_eq!(context.instructions.len(), 1);
        assert_eq!(context.instructions[0].source, "policy");
        assert_eq!(context.instructions[0].content, "be precise");
        assert_eq!(context.memory_snippets.len(), 1);
        assert_eq!(context.memory_snippets[0].source, "fact:repo");
    }
}
