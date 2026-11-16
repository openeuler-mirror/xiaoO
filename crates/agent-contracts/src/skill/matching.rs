//! L2 skill 发现的匹配契约（信号 / 证据 / 命中形态 + 匹配器 trait）。
//!
//! 契约与实现分离：本模块只定义跨 crate 的中立类型——实现（glob / 词法
//! 引擎）在 skill crate 的 `matching` 模块。契约放在 agent-contracts 的原因：
//! [`SkillRegistry`](super::SkillRegistry) 需要以 `skill_matcher()` 暴露预
//! 编译匹配器，而 skill crate 依赖 agent-contracts（依赖方向决定了类型只能
//! 定义在本侧）。
//!
//! 命中结果的 `summary` 借用自匹配器自身（`&self`），调用方在借用存活期
//! 内即时消费（注入文案 + 账本），不需要独立的数据生命周期。

use std::sync::Arc;

use agent_types::context::prompt::SkillSummary;

/// 本轮匹配信号集合。
pub struct SkillMatchSignals<'a> {
    /// 本迭代 Completed 写操作触及的文件（绝对或相对路径）
    pub written_paths: &'a [String],
    /// 用户输入信号：本次用户消息文本
    pub user_text: Option<&'a str>,
    /// 工作区根目录（write pivot 绝对路径剥根用；None 时按原文匹配）
    pub workspace_root: Option<&'a str>,
}

/// 单个命中的触发证据（L2 发现提示"触发原因"文案的素材）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillMatchReason {
    /// write pivot：写路径命中 `skill.paths` glob 模式
    Path { written: String, pattern: String },
    /// write pivot：写路径扩展名命中 tag
    Extension { written: String, tag: String },
    /// 用户输入文本命中 tag
    UserText { tag: String },
}

/// 命中结果：skill 摘要 + 触发证据（同一 skill 至多一条证据，
/// 取判定顺序中的首个命中信号）
pub struct SkillMatchHit<'a> {
    pub summary: &'a SkillSummary,
    pub reason: SkillMatchReason,
}

/// L2 匹配器契约：无状态、确定性（同一输入任意次调用同结果）。
/// registry 会话期持有、经 [`super::SkillRegistry::skill_matcher`] 跨 run
/// 共享；编译成本留在构建期是实现方的责任（见 skill crate 的 matching）。
pub trait SkillMatcher: Send + Sync {
    /// 判定哪些 skill 与当前活动相关，并携带触发证据（L2 提示文案用）。
    ///
    /// 返回按 `skill_id` 稳定排序的命中列表；两信号皆空时早退返回空。
    fn match_skills_detailed(&self, signals: &SkillMatchSignals<'_>) -> Vec<SkillMatchHit<'_>>;
}

/// 空匹配器：无 skill 清单 / 测试桩 / L2 关闭兜底，一切信号恒为空命中。
pub struct EmptySkillMatcher;

impl SkillMatcher for EmptySkillMatcher {
    fn match_skills_detailed(&self, _signals: &SkillMatchSignals<'_>) -> Vec<SkillMatchHit<'_>> {
        Vec::new()
    }
}

/// [`EmptySkillMatcher`] 的共享形态：`skill_matcher` 默认实现用，
/// 避免调用方各自 `Arc::new`。
pub fn empty_skill_matcher() -> Arc<dyn SkillMatcher> {
    Arc::new(EmptySkillMatcher)
}
