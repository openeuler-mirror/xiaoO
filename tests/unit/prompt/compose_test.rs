use super::*;
use agent_llm::ChatMessageExt;
use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
use agent_types::ChatMessage;

#[test]
fn channel_prompt_keeps_existing_rule_sections() {
    let prompt = compose_channel_system_prompt(ChannelPromptSections {
        memory_prompt: "<channel_memory>memo</channel_memory>",
        identity_prompt: "<participant_directory>people</participant_directory>",
        group_session_context: Some("recent context"),
    });

    assert!(prompt.contains("上下文边界与优先级"));
    assert!(prompt.contains("记忆写入指令"));
    assert!(prompt.contains("诚实与证据规则"));
    assert!(prompt.contains("群文件访问"));
    assert!(prompt.contains("participant_directory"));
    assert!(prompt.contains("channel_memory"));
}

#[test]
fn channel_prompt_output_matches_expected_template() {
    let prompt = compose_channel_system_prompt(ChannelPromptSections {
        memory_prompt: "<channel_memory>memo</channel_memory>",
        identity_prompt: "<participant_directory>people</participant_directory>",
        group_session_context: Some("recent context"),
    });

    let expected = [
        CHANNEL_CONTEXT_BOUNDARY_INSTRUCTION.trim(),
        CHANNEL_MEMORY_WRITE_INSTRUCTION.trim(),
        CHANNEL_HONESTY_INSTRUCTION.trim(),
        CHANNEL_FILE_INSTRUCTION.trim(),
        "## 当前群成员身份\n\n<participant_directory>people</participant_directory>",
        "## 长期记忆与稳定背景\n\n<channel_memory>memo</channel_memory>",
        "## 群聊背景片段（仅作参考，非当前问题本身）\n\nrecent context",
    ]
    .join("\n\n");

    assert_eq!(prompt, expected);
}

#[test]
fn generic_system_text_keeps_workspace_prompt_separate_and_skips_history_and_tools() {
    let context = PromptContext {
        environment: EnvironmentInfo {
            model: "gpt-test".to_string(),
            agent_id: "main".to_string(),
            cwd: "/tmp".to_string(),
            workspace_root: None,
            date: "2026-04-10".to_string(),
        },
        instructions: vec![crate::context::InstructionContext {
            source: "policy".to_string(),
            content: "be precise".to_string(),
        }],
        memory_snippets: vec![MemorySnippet {
            source: "fact:repo".to_string(),
            relevance_score: 0.9,
            content: "remember this".to_string(),
        }],
        skill_snippets: vec![SkillSummary {
            skill_id: "skill".to_string(),
            description: "do thing".to_string(),
        }],
        history: crate::context::CompressedHistory::from_messages(vec![ChatMessage::user("hello")]),
    };
    let stable = compose_system_sections(
        &format!(
            "base system\n\n{WORKSPACE_PROMPT_MARKER_BEGIN}\n## Workspace Instructions\n### /repo/AGENTS.md\nroot rules\n{WORKSPACE_PROMPT_MARKER_END}\n\n## 当前通道\n- 当前 channel: capture."
        ),
        &context,
    );

    // stable: [base (before+after merged), workspace_prompt, skill_section, environment]
    assert_eq!(stable.len(), 4);
    assert!(stable[0].starts_with("base system"));
    assert!(stable[0].contains("当前 channel: capture."));
    assert!(stable[2].contains("## Available Skills"));
    assert!(stable[2].contains("- skill: do thing"));
    assert!(stable[3].starts_with("## Environment"));
    assert!(stable[3].contains("- model: gpt-test"));

    // per-turn reminder: # Context (instructions + memory), tagged as a
    // system-reminder for message-tail injection — NOT part of the system
    // prompt, so the stable prefix stays byte-identical across turns.
    let reminder =
        compose_turn_context_reminder(&context).expect("turn context reminder should be present");
    assert!(reminder.starts_with("<system-reminder>"));
    assert!(reminder.ends_with("</system-reminder>"));
    assert!(reminder.contains("# Context"));
    assert!(!reminder.contains("## Environment"));
    assert!(reminder.contains("## Instructions"));
    assert!(reminder.contains("## Memory"));
    assert!(reminder.contains("[fact/repo] remember this"));
    assert!(reminder.contains("- policy: be precise"));
    assert!(!reminder.contains("score="));
    assert!(!reminder.contains("# Conversation"));
    assert!(!reminder.contains("# Tools"));

    assert!(stable[1].starts_with("## Workspace Instructions"));
    assert!(stable[1].contains("/repo/AGENTS.md"));
}

#[test]
fn repo_map_is_repositioned_after_available_skills() {
    let context = PromptContext {
        environment: EnvironmentInfo {
            model: "gpt-test".to_string(),
            agent_id: "main".to_string(),
            cwd: "/tmp".to_string(),
            workspace_root: None,
            date: "2026-04-10".to_string(),
        },
        instructions: Vec::new(),
        memory_snippets: Vec::new(),
        skill_snippets: vec![SkillSummary {
            skill_id: "commit-clean-code".to_string(),
            description: "verify code quality".to_string(),
        }],
        history: crate::context::CompressedHistory::from_messages(vec![ChatMessage::user("hello")]),
    };
    let base_system = format!(
        "You are a coding agent.\n\n## Skills System\n\nUse skills.\n\n{REPO_MAP_HEADER}\n\napps/main.rs\n  fn main()"
    );

    let text = compose_system_text(&base_system, &context);

    let skills_idx = text
        .find("### Available Skills")
        .expect("skills section must be present");
    let repo_idx = text
        .find("## Repository map")
        .expect("repo map must be present");
    assert!(
        repo_idx > skills_idx,
        "repo map must come after the skills catalog"
    );
}
