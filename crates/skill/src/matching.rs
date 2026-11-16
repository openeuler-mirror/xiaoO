//! L2 发现层的本地匹配引擎（纯函数、确定性、零外部状态）。
//!
//! 两个输入信号与 Claude Code 的双信号模型对齐：
//! - write pivot：本迭代 `Completed` 的 file_write/file_edit 触及的文件路径
//! - 用户输入信号：本次用户消息文本
//!
//! 账本过滤（`skill_hint_sent`）在 core 调用方完成——本模块不持有任何可变
//! 状态，同一输入任意调用产出相同结果。
//!
//! 成本纪律：匹配只在少数存在信号的轮次执行（无写路径且无非空白用户
//! 文本的迭代零成本早退）。用户文本截断到 [`MAX_USER_SIGNAL_SCALARS`]
//! 个 Unicode 标量并做一次小写化收集（全部 tag 匹配复用）——匹配成本与
//! 消息体积解耦，大日志粘贴不产生全量收集。tag 匹配为逐 tag 词边界包含
//! 检查（见 [`text_contains_tag_word`]）：子串安全，汉字等非 ASCII 字符
//! 天然充当词边界。glob pattern 在索引构建期同批预编译
//! （[`compile_patterns`]），热路径零编译。
//!
//! 类型契约（[`SkillMatchSignals`] / [`SkillMatchReason`] / [`SkillMatchHit`] /
//! [`SkillMatcher`]）定义在 agent-contracts，本模块实现之；[`SkillMatchIndex`]
//! **拥有**清单快照（`Arc<[SkillSummary]>`）与预编译 pattern，构建一次后
//! 由 registry 持有、经 `SkillRegistry::skill_matcher` 以 `Arc` 克隆跨 run
//! 复用——命中借用索引自身，不依赖调用方保证外部数据存活期。

use std::sync::Arc;

use agent_types::context::prompt::SkillSummary;

// 类型契约的权威定义在 agent-contracts；pub 再导出保持 `skill::` 路径完整
pub use agent_contracts::skill::matching::{
    SkillMatchHit, SkillMatchReason, SkillMatchSignals, SkillMatcher,
};

/// 用户输入信号的匹配上限（Unicode 标量数）。tag 命中只依赖词边界，
/// 常规触发词在消息前部已足够；超出部分不参与匹配。const 而非配置项：
/// 它是成本护栏而非行为开关，无需运行时调节（可见性与 core 侧的
/// `MAX_WHEN_TO_USE_HINT_SCALARS` 对齐：仅 crate 内部消费）
pub(crate) const MAX_USER_SIGNAL_SCALARS: usize = 8192;

/// 预编译匹配索引：拥有清单快照（与 registry 的 `list_skills` 共享同一
/// `Arc` 分配）与预编译 glob pattern，构建一次后由 registry 持有、经
/// `SkillRegistry::skill_matcher` 以 `Arc` 克隆跨 run 复用。tag 直接读
/// 共享快照的 `summary.tags`（零拷贝、无平行副本），走逐 tag 词边界扫描
/// （见 [`text_contains_tag_word`]），只在存在用户文本信号的轮次执行。
/// 命中结果借用 `&self`（索引存活于 registry，会话期不亡）。
pub struct SkillMatchIndex {
    /// 清单快照（与 registry 的 `list_skills` 共享同一 Arc 分配）
    summaries: Arc<[SkillSummary]>,
    /// 与 `summaries` 平行的预编译 glob pattern
    patterns: Vec<Vec<glob::Pattern>>,
}

impl SkillMatchIndex {
    /// 从共享清单快照构建：summaries 零拷贝共享，glob 预编译就地完成
    pub fn from_shared(summaries: Arc<[SkillSummary]>) -> Self {
        let patterns = summaries
            .iter()
            .map(|summary| compile_patterns(&summary.skill_id, &summary.paths))
            .collect();
        Self {
            summaries,
            patterns,
        }
    }

    /// 判定哪些 skill 与当前活动相关，并携带触发证据（L2 提示文案用）。
    ///
    /// 返回按 `skill_id` 稳定排序的命中列表；两信号皆空（无 write pivot
    /// 且无非空白用户文本）时早退返回空。
    pub fn match_skills_detailed(&self, signals: &SkillMatchSignals<'_>) -> Vec<SkillMatchHit<'_>> {
        let user_text_chars = prepared_user_text(signals);
        if signals.written_paths.is_empty() && user_text_chars.is_none() {
            return Vec::new();
        }
        // 无用户信号时为空切片：逐 tag 扫描对空文本天然产出空命中
        let user_text_chars = user_text_chars.unwrap_or_default();

        let mut matched: Vec<SkillMatchHit<'_>> = self
            .summaries
            .iter()
            .zip(self.patterns.iter())
            .filter_map(|(summary, patterns)| {
                entry_reason(signals, &user_text_chars, summary, patterns)
                    .map(|reason| SkillMatchHit { summary, reason })
            })
            .collect();

        matched.sort_by(|a, b| a.summary.skill_id.cmp(&b.summary.skill_id));
        matched
    }
}

impl SkillMatcher for SkillMatchIndex {
    fn match_skills_detailed(&self, signals: &SkillMatchSignals<'_>) -> Vec<SkillMatchHit<'_>> {
        // 固有方法优先解析，无递归风险
        SkillMatchIndex::match_skills_detailed(self, signals)
    }
}

/// 单个 skill 的证据判定：返回首个命中信号的证据（None = 不命中）。
/// 判定顺序单一来源：disable 过滤 → write 信号（逐写路径，先 glob 后
/// 扩展名 tag）→ 用户输入信号 tag。`user_text` 为预处理过的小写标量
/// 序列（空 = 无用户信号）；tag 直接取 `summary.tags`（共享快照原文）
fn entry_reason(
    signals: &SkillMatchSignals<'_>,
    user_text: &[char],
    summary: &SkillSummary,
    patterns: &[glob::Pattern],
) -> Option<SkillMatchReason> {
    // 模型无法调用的 skill 提示无意义
    if summary.disable_model_invocation {
        return None;
    }
    for written in signals.written_paths {
        for pattern in patterns {
            if path_matches_pattern(written, pattern, signals.workspace_root) {
                return Some(SkillMatchReason::Path {
                    written: written.clone(),
                    pattern: pattern.to_string(),
                });
            }
        }
        for tag in &summary.tags {
            if path_extension_matches_tag(written, tag) {
                return Some(SkillMatchReason::Extension {
                    written: written.clone(),
                    tag: tag.clone(),
                });
            }
        }
    }
    // 仅 write pivot 的轮次（无用户信号）不进入 tag 扫描，免去逐 tag
    // 小写化收集；空文本扫描恒为不命中，行为不变
    if user_text.is_empty() {
        return None;
    }
    for tag in &summary.tags {
        if text_contains_tag_word(user_text, tag) {
            return Some(SkillMatchReason::UserText { tag: tag.clone() });
        }
    }
    None
}

/// 用户文本以词边界形态包含 tag（保守命中，避免子串误报）。
/// 文本与 tag 在同一小写形态上比较（文本经 [`lowercase_chars`] 预处理，
/// tag 现场 ASCII 小写化）；边界判定 = 出现位置两侧为字符串端点或非
/// ASCII 单词字符（字母数字与 `_`）——汉字等非 ASCII 字符天然充当分隔符
/// （中文输入"帮我看下rs文件" 可命中 tag `rs`），ASCII 连续单词
/// （"parsers"、"foo_rs_bar"）不误报。空 tag 恒不命中
fn text_contains_tag_word(text: &[char], tag: &str) -> bool {
    let lowered: Vec<char> = tag.chars().map(|c| c.to_ascii_lowercase()).collect();
    if lowered.is_empty() || text.len() < lowered.len() {
        return false;
    }
    text.windows(lowered.len())
        .enumerate()
        .any(|(start, window)| {
            window == lowered.as_slice()
                && (start == 0 || !is_tag_word_char(text[start - 1]))
                && (start + lowered.len() == text.len()
                    || !is_tag_word_char(text[start + lowered.len()]))
        })
}

/// tag 词边界判定用的"单词字符"：ASCII 字母数字与下划线（与 \w 对齐，
/// `_` 属单词字符——`foo_rs_bar` 不命中 tag `rs`）
fn is_tag_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// 用户文本信号预处理：空白视为无信号（None），非空白截断到
/// [`MAX_USER_SIGNAL_SCALARS`] 个标量后小写化收集
fn prepared_user_text(signals: &SkillMatchSignals<'_>) -> Option<Vec<char>> {
    signals
        .user_text
        .filter(|text| !text.trim().is_empty())
        .map(|text| lowercase_chars(text, MAX_USER_SIGNAL_SCALARS))
}

/// glob pattern 编译；编译失败（无效 glob）直接丢弃，等同不命中。
/// 无效 pattern 打 warn——skill 的 `paths` 是第三方可写内容，静默丢弃
/// 会让触发器"配置了但不生效"且无从诊断
fn compile_patterns(skill_id: &str, paths: &[String]) -> Vec<glob::Pattern> {
    paths
        .iter()
        .filter_map(|pattern| match glob::Pattern::new(pattern) {
            Ok(compiled) => Some(compiled),
            Err(error) => {
                tracing::warn!(
                    skill_id,
                    pattern = %pattern,
                    %error,
                    "invalid skill path glob; this pattern will never match and is ignored"
                );
                None
            }
        })
        .collect()
}

/// glob 匹配：绝对路径先剥去 workspace_root 前缀再匹配，剥不掉（或无根可剥）
/// 则按原文匹配；两种形态都尝试，兼容绝对/相对两种 pattern 写法
fn path_matches_pattern(
    written: &str,
    pattern: &glob::Pattern,
    workspace_root: Option<&str>,
) -> bool {
    if pattern.matches(written) {
        return true;
    }
    if let Some(root) = workspace_root {
        let root = root.trim_end_matches('/');
        if !root.is_empty() {
            let suffix = written
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'));
            if let Some(relative) = suffix {
                return pattern.matches(relative);
            }
        }
    }
    false
}

/// 文件扩展名 ↔ tag（大小写不敏感）：裸扩展名本身命中，且常见语言的
/// 惯用别名拼写同样命中——`main.rs` 命中 tag `rs` **与** tag `rust`，
/// `main.py` 命中 `python`、`main.md` 命中 `markdown` 等。别名表只收录
/// 无歧义的惯用等价拼写，避免 tag 静默失效
fn path_extension_matches_tag(written: &str, tag: &str) -> bool {
    let Some(extension) = std::path::Path::new(written)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    let tag = tag.to_ascii_lowercase();
    extension == tag
        || extension_tag_aliases(&extension).is_some_and(|aliases| aliases.contains(&tag.as_str()))
}

/// 扩展名 → 惯用别名 tag（元素均小写）。收录标准：同语言无歧义的
/// 惯用拼写（`rust`↔`rs`、`python`↔`py`）；`h`（C/C++ 歧义）等不收录
fn extension_tag_aliases(extension: &str) -> Option<&'static [&'static str]> {
    match extension {
        "rs" => Some(&["rust"]),
        "py" => Some(&["python"]),
        "md" => Some(&["markdown"]),
        "js" => Some(&["javascript"]),
        "ts" => Some(&["typescript"]),
        "go" => Some(&["golang"]),
        "sh" | "bash" => Some(&["sh", "bash", "shell"]),
        "cpp" | "cc" | "cxx" => Some(&["cpp", "c++", "cxx"]),
        "yml" => Some(&["yaml"]),
        "rb" => Some(&["ruby"]),
        "kt" => Some(&["kotlin"]),
        _ => None,
    }
}

/// 用户文本的小写化标量序列，最多取前 `cap` 个（一次预处理，
/// 全部 tag 匹配复用；cap 见 [`MAX_USER_SIGNAL_SCALARS`]）
fn lowercase_chars(text: &str, cap: usize) -> Vec<char> {
    text.chars()
        .take(cap)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
#[path = "../../../tests/unit/skill/matching_test.rs"]
mod tests;
