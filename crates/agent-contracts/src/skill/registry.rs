use std::path::Path;
use std::sync::Arc;

use agent_types::context::prompt::SkillSummary;

use super::matching::{empty_skill_matcher, SkillMatcher};

/// Skill 执行模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillContext {
    /// 展开 prompt 到当前对话，LLM 继续用已有工具执行（默认）
    Inline,
    /// 启动独立子 agent 执行，独立 token 预算，不污染主对话
    Fork,
}

pub trait SkillRegistry: Send + Sync {
    /// 会话期不可变的清单快照（共享形态）：`Arc` 克隆零深拷贝，供
    /// prompt 清单段等逐轮消费方复用，避免每轮全量 clone。
    fn list_skills(&self) -> Arc<[SkillSummary]>;
    fn get_skill(&self, skill_id: &str) -> Option<&dyn SkillSpec>;
    /// L2 发现用的匹配器（语义见 [`SkillMatcher`]）。默认空匹配器：
    /// 无清单实现 / 测试桩的兜底，恒为空命中。
    fn skill_matcher(&self) -> Arc<dyn SkillMatcher> {
        empty_skill_matcher()
    }
}

pub trait SkillSpec: Send + Sync {
    // --- 基础信息 ---
    fn skill_id(&self) -> &str;
    fn description(&self) -> &str;
    fn full_prompt(&self) -> &str;

    /// 场景提示：什么情况下该用这个 skill（L2 发现提示文案用）。
    /// 与 description（清单展示）职责不同，允许缺省
    fn when_to_use(&self) -> Option<&str> {
        None
    }

    // --- 发现与匹配 ---

    /// 条件激活 glob 模式，只在 LLM 接触匹配文件时才出现在可用列表。空 = 始终可用
    fn paths(&self) -> &[String] {
        &[]
    }

    // --- 调用控制 ---

    /// 用户能否通过 /skill-name 手动调用。false = 仅 LLM 可调用
    fn user_invocable(&self) -> bool {
        true
    }
    /// true = 仅用户可触发，LLM 不会自动调用。适合危险操作
    fn disable_model_invocation(&self) -> bool {
        false
    }
    /// 命名参数列表，prompt 中 $arg_name 会被替换
    fn arguments(&self) -> &[String] {
        &[]
    }
    /// 参数提示文本，纯 UI 展示用
    fn argument_hint(&self) -> Option<&str> {
        None
    }

    // --- 执行环境 ---

    /// 执行模式：Inline（展开到当前对话）或 Fork（子 agent）
    fn context(&self) -> SkillContext {
        SkillContext::Inline
    }
    /// skill 文件所在目录
    fn location(&self) -> Option<&Path> {
        None
    }
}
