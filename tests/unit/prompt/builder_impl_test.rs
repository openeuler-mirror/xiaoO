use super::*;

use agent_types::common::ids::{ToolId, ToolName};
use agent_types::context::features::FeatureFlags;
use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use agent_types::TokenBudgetConfig;

struct TestToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
}

impl ToolSpecView for TestToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }
    fn name(&self) -> &ToolName {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }
    fn output_contract(&self) -> &OutputContract {
        static DEFAULT: OutputContract = OutputContract {
            description: String::new(),
        };
        &DEFAULT
    }
    fn effect_profile(&self) -> &EffectProfile {
        static DEFAULT: EffectProfile = EffectProfile {
            reads_filesystem: false,
            writes_filesystem: false,
            network_access: false,
            side_effects: false,
        };
        &DEFAULT
    }
}

#[test]
fn build_projects_tools_into_llm_request() {
    let builder = PromptBuilderImpl::new();
    let input = PromptBuildInput {
        system_prompt: "You are a coding agent.".to_string(),
        messages: vec![ChatMessage::user("hello")],
        visible_tools: vec![std::sync::Arc::new(TestToolSpec {
            id: ToolId("search".to_string()),
            name: ToolName("search".to_string()),
            description: "search docs".to_string(),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}}
                }),
            },
        }) as std::sync::Arc<dyn ToolSpecView>],
        skill_summaries: vec![SkillSummary {
            skill_id: "skill".to_string(),
            description: "do thing".to_string(),
        }],
        memory_snippets: vec![MemorySnippet {
            source: "memory".to_string(),
            content: "remember this".to_string(),
            relevance_score: 0.9,
        }],
        environment: EnvironmentInfo {
            model: "gpt-test".to_string(),
            cwd: "/tmp".to_string(),
            workspace_root: None,
            date: "2026-04-10".to_string(),
            agent_id: "main".to_string(),
        },
        feature_flags: FeatureFlags::default(),
        turn_count: 1,
        budget: TokenBudgetConfig {
            total_budget: 1024,
            reserved_for_output: 256,
            reserved_for_system: 128,
            hard_limit_ratio: 0.9,
        },
    };

    let result = futures::executor::block_on(builder.build(input)).unwrap();

    assert_eq!(result.request.tools.len(), 1);
    assert!(matches!(result.request.tool_choice, ToolChoice::Auto));
    assert!(matches!(
        result.request.response_format,
        ResponseFormat::Text
    ));
    assert_eq!(result.request.max_tokens, Some(256));
    assert!(matches!(
        result.request.messages[0].role,
        agent_types::MessageRole::System
    ));
    assert!(result.estimated_input_tokens > 0);
}

#[test]
fn build_appends_volatile_context_as_trailing_reminder_message() {
    let builder = PromptBuilderImpl::new();
    let input = PromptBuildInput {
        system_prompt: "You are a coding agent.".to_string(),
        messages: vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("working on it", 0),
            ChatMessage::user("continue"),
        ],
        visible_tools: Vec::new(),
        skill_summaries: Vec::new(),
        memory_snippets: vec![
            MemorySnippet {
                source: "horizon".to_string(),
                content: "- turn: 9/10 (1 remaining)".to_string(),
                relevance_score: 1.0,
            },
            MemorySnippet {
                source: "plan".to_string(),
                content: "[ ] fix the bug".to_string(),
                relevance_score: 0.9,
            },
        ],
        environment: EnvironmentInfo {
            model: "gpt-test".to_string(),
            cwd: "/tmp".to_string(),
            workspace_root: None,
            date: "2026-04-10".to_string(),
            agent_id: "main".to_string(),
        },
        feature_flags: FeatureFlags::default(),
        turn_count: 9,
        budget: TokenBudgetConfig {
            total_budget: 1024,
            reserved_for_output: 256,
            reserved_for_system: 128,
            hard_limit_ratio: 0.9,
        },
    };

    let result = futures::executor::block_on(builder.build(input)).unwrap();
    let messages = &result.request.messages;

    // [system, user, assistant, user, reminder]
    assert_eq!(messages.len(), 5);

    // The system message stays cache-stable: environment yes, per-turn
    // volatile context no.
    let system_text = messages[0].text_content().unwrap();
    assert!(matches!(messages[0].role, agent_types::MessageRole::System));
    assert!(system_text.contains("## Environment"));
    assert!(!system_text.contains("system-reminder"));
    assert!(!system_text.contains("## Progress"));
    assert!(!system_text.contains("## Active plan"));

    // The volatile context rides in an ephemeral trailing user message.
    let reminder = messages.last().unwrap();
    assert!(matches!(reminder.role, agent_types::MessageRole::User));
    let reminder_text = reminder.text_content().unwrap();
    assert!(reminder_text.starts_with("<system-reminder>"));
    assert!(reminder_text.ends_with("</system-reminder>"));
    assert!(reminder_text.contains("## Progress"));
    assert!(reminder_text.contains("- turn: 9/10 (1 remaining)"));
    assert!(reminder_text.contains("## Active plan"));
    assert!(reminder_text.contains("[ ] fix the bug"));

    // `system_parts` (fed to the system.transform hooker) excludes the
    // reminder as well.
    assert!(result
        .system_parts
        .iter()
        .all(|part| !part.contains("system-reminder")));
}

#[test]
fn build_fails_fast_when_budget_is_zero() {
    let builder = PromptBuilderImpl::new();
    let input = PromptBuildInput {
        system_prompt: "You are a coding agent.".to_string(),
        messages: vec![ChatMessage::user("hello")],
        visible_tools: Vec::new(),
        skill_summaries: Vec::new(),
        memory_snippets: Vec::new(),
        environment: EnvironmentInfo {
            model: "gpt-test".to_string(),
            cwd: String::new(),
            workspace_root: None,
            date: "2026-04-10".to_string(),
            agent_id: "main".to_string(),
        },
        feature_flags: FeatureFlags::default(),
        turn_count: 1,
        budget: TokenBudgetConfig {
            total_budget: 0,
            reserved_for_output: 0,
            reserved_for_system: 0,
            hard_limit_ratio: 1.0,
        },
    };

    let err = match futures::executor::block_on(builder.build(input)) {
        Ok(_) => panic!("expected prompt build to fail when budget is zero"),
        Err(err) => err,
    };

    assert!(matches!(err, PromptBuildError::BuildFailed { .. }));
}

#[test]
fn build_filters_out_existing_system_messages() {
    let builder = PromptBuilderImpl::new();
    let input = PromptBuildInput {
        system_prompt: "New system prompt".to_string(),
        messages: vec![
            ChatMessage::system("Old system prompt 1"),
            ChatMessage::user("hello"),
            ChatMessage::system("Old system prompt 2"),
            ChatMessage::assistant("response", 0),
        ],
        visible_tools: Vec::new(),
        skill_summaries: Vec::new(),
        memory_snippets: Vec::new(),
        environment: EnvironmentInfo {
            model: "gpt-test".to_string(),
            cwd: String::new(),
            workspace_root: None,
            date: "2026-04-10".to_string(),
            agent_id: "main".to_string(),
        },
        feature_flags: FeatureFlags::default(),
        turn_count: 1,
        budget: TokenBudgetConfig {
            total_budget: 1024,
            reserved_for_output: 256,
            reserved_for_system: 128,
            hard_limit_ratio: 0.9,
        },
    };

    let result = futures::executor::block_on(builder.build(input)).unwrap();

    assert_eq!(result.request.messages.len(), 3);

    assert_eq!(
        result.request.messages[0].role,
        agent_types::MessageRole::System
    );
    let system_text = result.request.messages[0].text_content().unwrap();
    assert!(system_text.contains("New system prompt"));
    assert!(system_text.contains("Environment"));
    assert!(
        !system_text.contains("Old system prompt"),
        "Old system messages should be filtered out"
    );

    assert_eq!(
        result.request.messages[1].role,
        agent_types::MessageRole::User
    );
    assert_eq!(result.request.messages[1].text_content(), Some("hello"));

    assert_eq!(
        result.request.messages[2].role,
        agent_types::MessageRole::Assistant
    );
    assert_eq!(result.request.messages[2].text_content(), Some("response"));
}
