use super::*;
use agent_llm::ChatMessageExt;

#[test]
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
