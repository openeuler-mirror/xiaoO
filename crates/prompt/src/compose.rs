use crate::context::PromptContext;

pub struct ChannelPromptSections<'a> {
    pub memory_prompt: &'a str,
    pub identity_prompt: &'a str,
    pub group_session_context: Option<&'a str>,
}

// Keep these markers in sync with apps/xiaoo-app/src/gateway/workspace_prompt.rs.
const WORKSPACE_PROMPT_MARKER_BEGIN: &str = "<xiaoo_workspace_prompt>";
const WORKSPACE_PROMPT_MARKER_END: &str = "</xiaoo_workspace_prompt>";

const CHANNEL_MEMORY_WRITE_INSTRUCTION: &str = r#"
## 记忆写入指令

你可以在回复中使用以下标记来保存重要信息到长期记忆：

1. **保存到群组记忆**（所有群成员可见）：
<save_to_group_memory>
这里写需要保存的群组重要信息，如群规、共同决定、重要事件等
</save_to_group_memory>

2. **保存到用户记忆**（仅当前用户可见）：
<save_to_user_memory>
这里写需要保存的用户个人信息，如用户偏好、个人事项等
</save_to_user_memory>

使用规则：
- 只有真正重要、需要长期记住的信息才写入记忆
- 不要写入临时性内容或普通对话
- 记忆内容要简洁、有价值
- 可以同时使用两种标记，也可以都不使用
- 凡是涉及群成员，必须使用 `<person uid="准确uid">用户名</person>` 标记人物
- 如果你要在飞书里真正 @ 某人，使用 `<at_user uid="准确uid">用户名</at_user>`
- 长期记忆在落盘时会保留 uid；面对用户回复时不要直接暴露 uid，只显示用户名
- 如果无法确定某个人的 uid，不要猜，直接要求用户补充 @ 或说明是谁
"#;

const CHANNEL_HONESTY_INSTRUCTION: &str = r#"
## 诚实与证据规则

- 只陈述当前会话、工具结果、对话历史、长期记忆或明确记录中有证据支持的事实
- 没有执行过的操作，不要说自己执行过；没有看到的文件、结果或记录，不要说自己已经看过
- 如果没有证据或记录支撑，就直接说明“目前没有证据”或“目前没有相关记录”
- 清楚区分事实、推断和不确定项，不要把猜测说成已经确认的事实
"#;

const CHANNEL_FILE_INSTRUCTION: &str = r#"
## 群文件访问

如果对话历史里出现了“文件ID:”和“落盘路径:”这样的群文件引用，表示文件正文没有被注入当前上下文。

当用户询问这些文件的内容、摘要、翻译、引用、章节、表格或结论时：
- 必须先调用 `channel_file_lookup` 工具读取或检索对应文件
- 不要假设 session 里已经有文件正文
- 优先使用文件ID 来定位文件
"#;

const CHANNEL_CONTEXT_BOUNDARY_INSTRUCTION: &str = r#"
## 上下文边界与优先级

你会同时看到几类上下文，它们的含义不同，不能混用：

1. 当前用户消息：
- 当前这条用户消息才是你现在要解决的问题，优先级最高
- 先回答当前问题，再决定是否需要参考下面的背景

2. 用户连续会话历史：
- 这是当前用户最近几轮对话，用来保持连续性
- 其中可能包含旧日期、旧地点、旧任务、旧搜索词
- 如果当前问题是“今天 / 现在 / 最新 / 实时”这类时效性问题，不要把历史里的旧日期或旧关键词直接当成当前查询条件

3. 群组记忆 / 用户记忆：
- 这是长期记忆，适合提供稳定背景，如偏好、群规、长期决定
- 长期记忆不是“当前问题本身”，也不等于实时事实

4. 相关群聊记录：
- 这是为当前问题补充的群聊背景片段
- 若是“引用命中的群聊上下文”，它表示用户明确引用的历史消息，可信度高于普通最近片段
- 若是“最近群聊片段”，它只是背景参考，不能机械地把里面偶然出现的旧日期、旧地点、旧数字、旧关键词拿去驱动当前搜索

5. 被引用消息的归属：
- 如果“引用命中的群聊上下文”里明确标注“已触发当前助手处理链”，表示那条历史消息原本就是发给当前助手的
- 这种情况下，即使那条历史消息里出现了 `@名称`，也不要把它误解成“应该等第三方来回答”
- 只有当前用户明确要求“让他来回”“等他回复”“帮我转告某人”时，才把问题理解为要第三方来回答

当不同上下文之间有冲突时，使用下面的优先级：
- 当前用户消息
- 当前消息显式引用的内容
- 当前用户最近连续会话
- 长期记忆
- 普通最近群聊片段

如果当前问题需要联网、搜索或工具查询：
- 查询词优先来自当前用户消息的明确要求
- 不要因为历史上下文里出现过某个年份、日期、城市或关键词，就自动把它拼进本轮搜索
- 除非用户明确要求查询某段历史时间，否则不要把旧时间条件带入“今天 / 现在 / 最新”类问题
"#;

pub fn compose_system_messages(base_system: &str, context: &PromptContext) -> Vec<String> {
    let (base_system, workspace_prompt) = split_workspace_prompt_block(base_system);
    let mut messages = Vec::new();

    let base_system = base_system.trim();
    if !base_system.is_empty() {
        messages.push(base_system.to_string());
    }

    if let Some(context_section) = compose_context_section(context) {
        messages.push(context_section);
    }

    if let Some(workspace_prompt) = workspace_prompt {
        messages.push(workspace_prompt);
    }

    messages
}

pub fn compose_system_text(base_system: &str, context: &PromptContext) -> String {
    compose_system_messages(base_system, context).join("\n\n")
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

fn compose_context_section(context: &PromptContext) -> Option<String> {
    let mut sections = Vec::new();

    if let Some(environment_section) = compose_environment_section(context) {
        sections.push(environment_section);
    }
    if let Some(instruction_section) = compose_instruction_section(context) {
        sections.push(instruction_section);
    }
    if let Some(memory_section) = compose_memory_section(context) {
        sections.push(memory_section);
    }
    if let Some(skill_section) = compose_skill_section(context) {
        sections.push(skill_section);
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!("# Context\n{}", sections.join("\n\n")))
    }
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

fn compose_instruction_section(context: &PromptContext) -> Option<String> {
    if context.instructions.is_empty() {
        return None;
    }

    let lines = context
        .instructions
        .iter()
        .map(|instruction| {
            format!(
                "- {}: {}",
                instruction.source.trim(),
                instruction.content.trim()
            )
        })
        .collect::<Vec<_>>();

    Some(format!("## Instructions\n{}", lines.join("\n")))
}

fn compose_memory_section(context: &PromptContext) -> Option<String> {
    if context.memory_snippets.is_empty() {
        return None;
    }

    let lines = context
        .memory_snippets
        .iter()
        .map(|snippet| {
            format!(
                "- [{}] {}",
                normalize_memory_source(&snippet.source),
                snippet.content.trim()
            )
        })
        .collect::<Vec<_>>();

    Some(format!("## Memory\n{}", lines.join("\n")))
}

fn compose_skill_section(context: &PromptContext) -> Option<String> {
    if context.skill_snippets.is_empty() {
        return None;
    }

    let lines = context
        .skill_snippets
        .iter()
        .map(|snippet| {
            format!(
                "- {}: {}",
                snippet.skill_id.trim(),
                snippet.description.trim()
            )
        })
        .collect::<Vec<_>>();

    let mut section = String::from("## Available Skills\nThe following skills are available. Use the `skill` tool to invoke them by name.\n");
    section.push_str(&lines.join("\n"));
    Some(section)
}

fn normalize_memory_source(source: &str) -> String {
    let source = source.trim();
    if let Some(value) = source.strip_prefix("fact:") {
        format!("fact/{}", value.trim())
    } else {
        source.to_string()
    }
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
