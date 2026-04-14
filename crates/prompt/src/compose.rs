use std::sync::Arc;

use agent_contracts::ToolSpecView;
use agent_types::{ChatMessage, ContentBlock, MessageRole};

use crate::context::PromptContext;

pub struct ChannelPromptSections<'a> {
    pub memory_prompt: &'a str,
    pub identity_prompt: &'a str,
    pub group_session_context: Option<&'a str>,
}

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

pub fn compose_system_text(
    base_system: &str,
    context: &PromptContext,
    tools: &[Arc<dyn ToolSpecView>],
) -> String {
    let mut sections = Vec::new();

    let base_system = base_system.trim();
    if !base_system.is_empty() {
        sections.push(base_system.to_string());
    }

    if let Some(context_section) = compose_context_section(context) {
        sections.push(context_section);
    }
    if let Some(conversation_section) = compose_conversation_section(context) {
        sections.push(conversation_section);
    }
    if let Some(tool_section) = compose_tool_section(tools) {
        sections.push(tool_section);
    }

    sections.join("\n\n")
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

fn compose_conversation_section(context: &PromptContext) -> Option<String> {
    let mut sections = Vec::new();

    if let Some(summary) = context.history.summary.as_deref() {
        let summary = summary.trim();
        if !summary.is_empty() {
            sections.push(format!("## Summary\n{}", summary));
        }
    }

    if !context.history.compressed_messages.is_empty() {
        sections.push(format!(
            "## Compressed Messages\n{}",
            render_history_messages(&context.history.compressed_messages)
        ));
    }

    if !context.history.recent_tail.is_empty() {
        sections.push(format!(
            "## Recent Tail\n{}",
            render_history_messages(&context.history.recent_tail)
        ));
    }

    if sections.is_empty() {
        None
    } else {
        Some(format!("# Conversation\n{}", sections.join("\n\n")))
    }
}

fn compose_tool_section(tools: &[Arc<dyn ToolSpecView>]) -> Option<String> {
    if tools.is_empty() {
        return None;
    }

    let lines = tools
        .iter()
        .map(|tool| {
            format!(
                "## {}\n- description: {}\n- input_schema:\n```json\n{}\n```",
                tool.name().0.trim(),
                tool.description().trim(),
                serde_json::to_string_pretty(&tool.input_schema().schema).unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();

    Some(format!("# Tools\n{}", lines.join("\n\n")))
}

fn render_history_messages(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .map(render_history_message)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_history_message(message: &ChatMessage) -> String {
    let role = match message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };

    let content = message
        .blocks
        .iter()
        .map(render_content_block)
        .collect::<Vec<_>>()
        .join(" | ");

    format!("- {}: {}", role, content.trim())
}

fn render_content_block(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text } => text.trim().to_string(),
        ContentBlock::ToolUse {
            tool_name, input, ..
        } => format!(
            "[tool_use {} {}]",
            tool_name.trim(),
            serde_json::to_string(input).expect("tool input must be serializable"),
        ),
        ContentBlock::ToolResult {
            tool_name,
            output,
            is_error,
            ..
        } => format!(
            "[tool_result {} error={} {}]",
            tool_name.trim(),
            is_error,
            output.trim(),
        ),
        ContentBlock::Image { description } => format!("[image {}]", description.trim()),
        ContentBlock::Document { description } => format!("[document {}]", description.trim()),
    }
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

    use agent_types::common::ids::{ToolId, ToolName};
    use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
    use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

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
    fn generic_system_text_uses_stable_section_order() {
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
        let tools = vec![Arc::new(TestToolSpec {
            id: ToolId("search".to_string()),
            name: ToolName("search".to_string()),
            description: "search docs".to_string(),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}}
                }),
            },
        }) as Arc<dyn ToolSpecView>];

        let text = compose_system_text("base system", &context, &tools);

        assert!(text.find("# Context").unwrap() > text.find("base system").unwrap());
        assert!(text.find("## Environment").unwrap() > text.find("# Context").unwrap());
        assert!(text.find("## Instructions").unwrap() > text.find("## Environment").unwrap());
        assert!(text.find("## Memory").unwrap() > text.find("## Instructions").unwrap());
        assert!(text.find("## Skills").unwrap() > text.find("## Memory").unwrap());
        assert!(text.find("# Conversation").unwrap() > text.find("# Context").unwrap());
        assert!(text.find("# Tools").unwrap() > text.find("# Conversation").unwrap());
        assert!(text.contains("[fact/repo] remember this"));
        assert!(text.contains("- policy: be precise"));
        assert!(!text.contains("score="));
    }
}
