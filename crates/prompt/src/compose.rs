use crate::context::PromptContext;

pub struct ChannelPromptSections<'a> {
    pub memory_prompt: &'a str,
    pub identity_prompt: &'a str,
    pub group_session_context: Option<&'a str>,
}

// Keep these markers in sync with apps/xiaoo-app/src/gateway/workspace_prompt.rs.
const WORKSPACE_PROMPT_MARKER_BEGIN: &str = "<xiaoo_workspace_prompt>";
const WORKSPACE_PROMPT_MARKER_END: &str = "</xiaoo_workspace_prompt>";

const CHANNEL_MEMORY_WRITE_INSTRUCTION: &str =
    include_str!("prompts/channel_memory_write_instruction.md");
const CHANNEL_HONESTY_INSTRUCTION: &str = include_str!("prompts/channel_honesty_instruction.md");
const CHANNEL_FILE_INSTRUCTION: &str = include_str!("prompts/channel_file_instruction.md");
const CHANNEL_CONTEXT_BOUNDARY_INSTRUCTION: &str =
    include_str!("prompts/channel_context_boundary_instruction.md");

pub(crate) fn compose_system_messages(base_system: &str, context: &PromptContext) -> Vec<String> {
    let (base_system, workspace_prompt) = split_workspace_prompt_block(base_system);
    let mut messages = Vec::new();

    // Stable prefix first: the base system prompt, workspace rules, and skill
    // catalog rarely change, so they form a cache-friendly span. The volatile
    // `# Context` block (date, remaining horizon, live plan) used to sit at the
    // front, where its changing first bytes shifted the whole prefix and defeated
    // provider prefix caching; it is now appended last.
    let base_system = base_system.trim();
    if !base_system.is_empty() {
        messages.push(base_system.to_string());
    }

    if let Some(workspace_prompt) = workspace_prompt {
        messages.push(workspace_prompt);
    }

    if let Some(skill_section) = compose_skill_section(context) {
        messages.push(skill_section);
    }

    // Volatile tail last — environment, remaining-horizon notice, active plan,
    // and recalled memory change turn-to-turn and must stay out of the cached prefix.
    if let Some(context_section) = compose_context_section(context) {
        messages.push(context_section);
    }

    messages
}

/// The per-turn dynamic block: environment, remaining-budget horizon, the live
/// plan from the `todo_write` store, and recalled memory. Kept whole and last so
/// the stable prefix above stays cache-aligned.
fn compose_context_section(context: &PromptContext) -> Option<String> {
    let mut blocks = Vec::new();
    if let Some(section) = compose_environment_section(context) {
        blocks.push(section);
    }
    if let Some(section) = compose_progress_section(context) {
        blocks.push(section);
    }
    if let Some(section) = compose_plan_section(context) {
        blocks.push(section);
    }
    if let Some(section) = compose_instructions_section(context) {
        blocks.push(section);
    }
    if let Some(section) = compose_memory_section(context) {
        blocks.push(section);
    }
    if blocks.is_empty() {
        None
    } else {
        Some(format!("# Context\n\n{}", blocks.join("\n\n")))
    }
}

/// Remaining-horizon notice: turns and tokens consumed so far, with a wrap-up
/// nudge as the turn cap approaches so the model commits its best partial result.
/// Rendered from the `horizon` snippet the agent loop injects each turn.
fn compose_progress_section(context: &PromptContext) -> Option<String> {
    let snippet = context
        .memory_snippets
        .iter()
        .find(|snippet| snippet.source == "horizon")?;
    let content = snippet.content.trim();
    if content.is_empty() {
        return None;
    }
    Some(format!("## Progress\n{}", content))
}

/// Live plan: the open `todo_write` items, re-injected every turn so plan state
/// is load-bearing context rather than a write-only decoration.
fn compose_plan_section(context: &PromptContext) -> Option<String> {
    let items: Vec<&str> = context
        .memory_snippets
        .iter()
        .filter(|snippet| snippet.source == "plan")
        .map(|snippet| snippet.content.trim())
        .filter(|content| !content.is_empty())
        .collect();
    if items.is_empty() {
        return None;
    }
    let mut section = String::from(
        "## Active plan\nOpen items from your todo list — keep working until each is done or you update the plan with todo_write:",
    );
    for item in items {
        section.push_str("\n- ");
        section.push_str(item);
    }
    Some(section)
}

pub fn compose_system_text(base_system: &str, context: &PromptContext) -> String {
    compose_system_messages(base_system, context).join("\n\n")
}

/// Split the system prompt into a cache-stable prefix (base prompt, workspace
/// rules, skills catalog) and the per-turn-volatile `# Context` tail (environment,
/// horizon, plan, memory). Emitting them as two distinct system blocks lets a
/// provider with an explicit cache breakpoint (Anthropic) cache the stable prefix
/// without the volatile tail invalidating it every turn — the same goal the
/// stable-prefix ordering already serves for providers that auto-cache the
/// longest matching prefix. Returns `(stable, volatile?)`.
pub(crate) fn compose_system_parts(
    base_system: &str,
    context: &PromptContext,
) -> (String, Option<String>) {
    let (base_system, workspace_prompt) = split_workspace_prompt_block(base_system);
    let mut stable = Vec::new();
    let base_system = base_system.trim();
    if !base_system.is_empty() {
        stable.push(base_system.to_string());
    }
    if let Some(workspace_prompt) = workspace_prompt {
        stable.push(workspace_prompt);
    }
    if let Some(skill_section) = compose_skill_section(context) {
        stable.push(skill_section);
    }
    let volatile = compose_context_section(context);
    (stable.join("\n\n"), volatile)
}

pub fn compose_channel_system_prompt(sections: ChannelPromptSections<'_>) -> String {
    let mut parts = vec![
        CHANNEL_CONTEXT_BOUNDARY_INSTRUCTION.trim().to_string(),
        CHANNEL_MEMORY_WRITE_INSTRUCTION.trim().to_string(),
        CHANNEL_HONESTY_INSTRUCTION.trim().to_string(),
        CHANNEL_FILE_INSTRUCTION.trim().to_string(),
    ];

    let identity_prompt = sections.identity_prompt.trim();
    if !identity_prompt.is_empty() {
        parts.push(format!("## 当前群成员身份\n\n{}", identity_prompt));
    }

    let memory_prompt = sections.memory_prompt.trim();
    if !memory_prompt.is_empty() {
        parts.push(format!("## 长期记忆与稳定背景\n\n{}", memory_prompt));
    }

    if let Some(group_session_context) = sections.group_session_context {
        let group_session_context = group_session_context.trim();
        if !group_session_context.is_empty() {
            parts.push(format!(
                "## 群聊背景片段（仅作参考，非当前问题本身）\n\n{}",
                group_session_context
            ));
        }
    }

    parts.join("\n\n")
}

fn split_workspace_prompt_block(base_system: &str) -> (String, Option<String>) {
    let base_system = base_system.trim();
    let Some(start_index) = base_system.find(WORKSPACE_PROMPT_MARKER_BEGIN) else {
        return (base_system.to_string(), None);
    };

    let workspace_start = start_index + WORKSPACE_PROMPT_MARKER_BEGIN.len();
    let Some(relative_end_index) = base_system[workspace_start..].find(WORKSPACE_PROMPT_MARKER_END)
    else {
        return (base_system.to_string(), None);
    };
    let workspace_end = workspace_start + relative_end_index;

    let workspace_prompt = base_system[workspace_start..workspace_end].trim();
    let before = base_system[..start_index].trim();
    let after = base_system[workspace_end + WORKSPACE_PROMPT_MARKER_END.len()..].trim();

    let mut remaining_sections = Vec::new();
    if !before.is_empty() {
        remaining_sections.push(before.to_string());
    }
    if !after.is_empty() {
        remaining_sections.push(after.to_string());
    }

    (
        remaining_sections.join("\n\n"),
        (!workspace_prompt.is_empty()).then(|| workspace_prompt.to_string()),
    )
}

fn compose_environment_section(context: &PromptContext) -> Option<String> {
    let mut lines = Vec::new();

    if !context.environment.model.trim().is_empty() {
        lines.push(format!("- model: {}", context.environment.model));
    }
    if !context.environment.agent_id.trim().is_empty() {
        lines.push(format!("- agent_id: {}", context.environment.agent_id));
    }
    if !context.environment.date.trim().is_empty() {
        lines.push(format!("- today: {}", context.environment.date));
    }

    if !context.environment.cwd.trim().is_empty() {
        lines.push(format!("- cwd: {}", context.environment.cwd));
    }

    if let Some(workspace_root) = context.environment.workspace_root.as_deref() {
        if !workspace_root.trim().is_empty() {
            lines.push(format!("- workspace_root: {}", workspace_root));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(format!("## Environment\n{}", lines.join("\n")))
    }
}

/// Recalled memory: facts the agent loop pins into context, re-surfaced every
/// turn. Excludes the `horizon`/`plan` snippets, which render in their own sections.
fn compose_memory_section(context: &PromptContext) -> Option<String> {
    let lines: Vec<String> = context
        .memory_snippets
        .iter()
        .filter(|snippet| snippet.source != "horizon" && snippet.source != "plan")
        .filter(|snippet| !snippet.content.trim().is_empty())
        .map(|snippet| {
            let label = snippet.source.trim().replace(':', "/");
            if label.is_empty() {
                snippet.content.trim().to_string()
            } else {
                format!("[{}] {}", label, snippet.content.trim())
            }
        })
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!("## Memory\n{}", lines.join("\n")))
}

fn compose_instructions_section(context: &PromptContext) -> Option<String> {
    if context.instructions.is_empty() {
        return None;
    }
    let lines: Vec<String> = context
        .instructions
        .iter()
        .map(|instruction| {
            format!(
                "- {}: {}",
                instruction.source.trim(),
                instruction.content.trim()
            )
        })
        .collect();
    Some(format!("## Instructions\n{}", lines.join("\n")))
}

fn compose_skill_section(context: &PromptContext) -> Option<String> {
    if context.skill_snippets.is_empty() {
        return None;
    }

    let mut lines = Vec::new();

    for snippet in &context.skill_snippets {
        let skill_id = snippet.skill_id.trim();
        let desc = snippet.description.trim();

        lines.push(format!("- {}: {}", skill_id, desc));
    }

    let mut section = String::from("### Available Skills\n\nInvoke skills by name using `skill` tool or `/skill-name` command.\n");
    section.push_str(&lines.join("\n"));
    Some(section)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::ChatMessageExt;
    use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
    use agent_types::ChatMessage;

    #[test]
    #[ignore]
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
    #[ignore]
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
    #[ignore]
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
            history: crate::context::CompressedHistory::from_messages(vec![ChatMessage::user(
                "hello",
            )]),
        };
        let messages = compose_system_messages(
            &format!(
                "base system\n\n{WORKSPACE_PROMPT_MARKER_BEGIN}\n## Workspace Instructions\n### /repo/AGENTS.md\nroot rules\n{WORKSPACE_PROMPT_MARKER_END}\n\n## 当前通道\n- 当前 channel: capture."
            ),
            &context,
        );

        assert_eq!(messages.len(), 3);
        assert!(messages[0].starts_with("base system"));
        assert!(messages[0].contains("当前 channel: capture."));
        assert!(messages[1].starts_with("# Context"));
        assert!(messages[1].contains("## Environment"));
        assert!(messages[1].contains("## Instructions"));
        assert!(messages[1].contains("## Memory"));
        assert!(messages[1].contains("## Available Skills"));
        assert!(messages[2].starts_with("## Workspace Instructions"));
        assert!(messages[2].contains("/repo/AGENTS.md"));
        assert!(messages[1].contains("[fact/repo] remember this"));
        assert!(messages[1].contains("- policy: be precise"));
        assert!(!messages[1].contains("score="));
        assert!(!messages[1].contains("# Conversation"));
        assert!(!messages[1].contains("# Tools"));
    }
}
