//! L2 skill 动态发现层的注入链路（write pivot + 用户输入信号）。
//!
//! 分层契约（见 plan.md §1.1）：
//! - 匹配判定全部委托 registry 暴露的匹配器（`Arc<dyn SkillMatcher>`）；
//!   本模块只负责门控、账本过滤、消息注入与记账。
//! - 提示消息是 `<system-reminder>` 包裹的 user 消息，持久化进历史
//!   （append-only，不破坏前缀缓存）；**不过** `apply_chat_message_hook`
//!   ——运行时注入而非用户撰写（同 streak_note 先例）。
//! - 账本（`LoopState::skill_hint_sent`，语义见该字段注释）过滤已出现
//!   在历史的 skill；压缩时由 `compress` 清空。
//! - 生效门控：`skill_matching && skill_discovery`（L2 以 L1 清单开启为
//!   前提——L1 关闭时清单段被清空，引用清单段的 L2 提示必须联动关闭）。
//! - 初始用户消息的用户输入信号**不在 push 时注入**，而是在 run 首轮、
//!   checkpoint `process_pending` 之后结算（见 `run_agent_loop`）：restore
//!   以"历史最后一条消息"为新输入回插（driver 的 new_input 捕获），
//!   push 后立即注入会让 restore 误把 L2 提示当用户输入回插、真实用户
//!   消息被静默丢弃。drain 消息无此交互（push 发生在 process_pending
//!   之后），push 后即注入。
//! - write pivot 的 suspended 协议守卫：suspended 批次本轮不注入，信号
//!   存入 `LoopState::pending_write_pivot`，由恢复 run 的首轮（resumer
//!   已把全部 tool_result 插回、且 checkpoint `process_pending` 已执行
//!   之后）补偿注入——restore 先行，刚注入的提示不会被随后的历史回滚
//!   冲掉；守卫期间不产生 user 消息隔断，信号也不丢失。

use std::collections::HashSet;
use std::sync::Arc;

use agent_contracts::skill::matching::{SkillMatchHit, SkillMatchReason, SkillMatchSignals};
use agent_contracts::skill::SkillMatcher;
use agent_contracts::RuntimeView;
use agent_llm::ChatMessageExt;
use agent_types::common::{sanitize_prompt_field, truncate_scalars};
use agent_types::context::FeatureFlags;
use agent_types::tool::{RawToolOutcome, ToolExecutionResult};
use agent_types::ChatMessage;
use serde_json::Value;

use crate::agent_loop::LoopContext;
use crate::loop_state::LoopState;
use crate::suspend::SuspendedToolCall;
use crate::token_estimator::TokenEstimator;

/// L2 提示内 `when_to_use` 的展示上限（Unicode 标量数）。const 而非配置项：
/// 与 skill crate 的 `MAX_USER_SIGNAL_SCALARS` 同理，它是成本护栏——skill
/// 元数据是第三方可写内容，无界 `when_to_use` 会随每次命中放大提示与
/// 历史体积，无需运行时调节（截断只作用于 prompt 视图，见
/// `truncate_scalars` 的模块 doc）
pub(crate) const MAX_WHEN_TO_USE_HINT_SCALARS: usize = 200;

/// L2 发现门控：`skill_matching`（L1 清单）开启是 L2 的前提
pub fn skill_discovery_enabled(flags: &FeatureFlags) -> bool {
    flags.skill_matching && flags.skill_discovery
}

/// run 级发现视图：matcher 经 `SkillRegistry::skill_matcher()` 共享，
/// 本视图只做 flags 门控与账本过滤。门控失败即不持有引用
/// （`inner = None`，一切匹配调用零成本返回空）。
pub struct SkillDiscoveryView {
    inner: Option<Arc<dyn SkillMatcher>>,
}

impl SkillDiscoveryView {
    pub fn new(flags: &FeatureFlags, matcher: Arc<dyn SkillMatcher>) -> Self {
        Self {
            inner: skill_discovery_enabled(flags).then_some(matcher),
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// 指定信号的匹配（两个注入入口共用），结果经账本过滤。
    ///
    /// 门控与空信号早退检查由公开入口
    /// [`inject_user_signal_skill_hints`] / [`inject_write_pivot_skill_hints`]
    /// 承担，本私有辅助信任调用方已通过。
    fn signal_hits(
        &self,
        sent: &HashSet<String>,
        signals: &SkillMatchSignals<'_>,
    ) -> Vec<SkillMatchHit<'_>> {
        let Some(matcher) = self.inner.as_ref() else {
            return Vec::new();
        };
        fresh_hits(sent, matcher.match_skills_detailed(signals))
    }
}

/// 账本过滤：已提示过 / 已成功调用过（全文已在历史）的 skill 不再注入
fn fresh_hits<'h>(sent: &HashSet<String>, hits: Vec<SkillMatchHit<'h>>) -> Vec<SkillMatchHit<'h>> {
    hits.into_iter()
        .filter(|hit| !sent.contains(&hit.summary.skill_id))
        .collect()
}

/// 初始用户消息入史后的 L2 发现结算素材（`append_initial_user_message`
/// 产出；消费点见 [`settle_initial_user_message_l2`]）。
pub(crate) struct InitialUserMessageL2 {
    /// `/skill-name` 展开且全文确实随消息入史 → 待记账的规范 skill id
    /// （命令 deny 早返回 / 命令与 chat hook 重写路径已在准备阶段剔除
    /// 记账资格——全文未入史就记账会永久压制后续发现提示）
    pub(crate) expanded_skill_id: Option<String>,
    /// 用户输入信号文本（终态、模型可见形态）。skill 展开消息是 skill
    /// 全文而非用户意图，不产生信号（Claude skipSkillDiscovery 门）
    pub(crate) signal_text: Option<String>,
}

/// 初始用户消息的 L2 结算（`/skill-name` 展开记账 + 用户输入信号注入）。
/// run 首轮、checkpoint `process_pending` 之后调用一次（时序理由见
/// 模块 doc）。
pub(crate) fn settle_initial_user_message_l2(ctx: &mut LoopContext<'_>, l2: InitialUserMessageL2) {
    if let Some(skill_id) = l2.expanded_skill_id {
        ctx.state.skill_hint_sent.insert(skill_id);
    }
    if let Some(text) = l2.signal_text {
        inject_user_signal_skill_hints(ctx.state, ctx.estimator, &ctx.skill_discovery, &text);
    }
}

/// 上一 run 被协议守卫推迟的 write pivot 信号补偿。恢复 run 首轮、
/// checkpoint `process_pending` 之后调用一次（时序理由见模块 doc）；
/// 消费即清，无论命中与否（防陈旧信号跨 run 滞留）
pub(crate) fn compensate_pending_write_pivot(ctx: &mut LoopContext<'_>) {
    if ctx.state.pending_write_pivot.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut ctx.state.pending_write_pivot);
    let workspace_root = workspace_root_of(ctx.input.runtime_view.as_ref());
    inject_write_pivot_skill_hints(
        ctx.state,
        ctx.estimator,
        &ctx.skill_discovery,
        &pending,
        workspace_root.as_deref(),
    );
}

/// 用户输入信号注入。两个调用入口：
/// - 初始用户消息：经 [`settle_initial_user_message_l2`] 延迟结算
/// - drain 消息：push 之后立即调用
///
/// 入口时序差异的理由见模块 doc。命中且不在账本 → 紧随用户消息追加
/// L2 提示（不过 chat hook），skill 名入账本。
pub(crate) fn inject_user_signal_skill_hints(
    state: &mut LoopState,
    estimator: &TokenEstimator,
    discovery: &SkillDiscoveryView,
    user_text: &str,
) {
    if !discovery.is_enabled() || user_text.trim().is_empty() {
        return;
    }
    let signals = SkillMatchSignals {
        written_paths: &[],
        user_text: Some(user_text),
        workspace_root: None,
    };
    let hits = discovery.signal_hits(&state.skill_hint_sent, &signals);
    if hits.is_empty() {
        return;
    }
    push_skill_hint_message(state, estimator, &hits);
}

/// write pivot 注入：`record_tool_results` 尾部（工具结果全部入史后）与
/// 恢复入口（suspended 补偿）共用。suspended 协议守卫由调用方承担
/// （守卫理由见模块 doc）。
///
/// 只负责匹配与注入；本批成功调用的 skill 记账由调用方在注入判定前
/// 统一完成（单一记账点，见 `record_tool_results`）。
pub(crate) fn inject_write_pivot_skill_hints(
    state: &mut LoopState,
    estimator: &TokenEstimator,
    discovery: &SkillDiscoveryView,
    written_paths: &[String],
    workspace_root: Option<&str>,
) {
    if !discovery.is_enabled() {
        return;
    }
    // 零成本早退红线：written_paths 为空即不做任何匹配工作
    if written_paths.is_empty() {
        return;
    }
    let signals = SkillMatchSignals {
        written_paths,
        user_text: None,
        workspace_root,
    };
    let hits = discovery.signal_hits(&state.skill_hint_sent, &signals);
    if hits.is_empty() {
        return;
    }
    push_skill_hint_message(state, estimator, &hits);
}

fn push_skill_hint_message(
    state: &mut LoopState,
    estimator: &TokenEstimator,
    hits: &[SkillMatchHit<'_>],
) {
    let reminder = compose_skill_hint_message(hits);
    let mut message = ChatMessage::user(reminder);
    estimator.update_message_cache(&mut message);
    state.messages.write().push(message);
    for hit in hits {
        state.skill_hint_sent.insert(hit.summary.skill_id.clone());
    }
    tracing::debug!(
        skills = ?hits
            .iter()
            .map(|hit| hit.summary.skill_id.as_str())
            .collect::<Vec<_>>(),
        "injected L2 skill discovery hints"
    );
}

/// L2 提示消息文案（user 角色，`<system-reminder>` 包裹）。
///
/// 每行 = name + when_to_use（缺省省略，超 [`MAX_WHEN_TO_USE_HINT_SCALARS`]
/// 个标量时确定性截断）+ 触发原因；**绝不包含 description**——那是 L1
/// 清单段的专属信息（信息分层红线）。
///
/// 可信边界：skill 元数据（skill_id / when_to_use / paths glob / tags）是
/// 第三方可写内容，插值前一律过 [`sanitize_prompt_field`]（agent-types
/// 共享实现，L1 清单段同源）——闭合标签 `</system-reminder>` 会逃逸包裹
/// 层、换行可伪造消息结构。账本与 `/skill-name` 展开仍使用原始 skill_id
/// （逐字一致）；清洗只作用于本提示的展示文本，不影响任何记账键。
pub(crate) fn compose_skill_hint_message(hits: &[SkillMatchHit<'_>]) -> String {
    let mut body = String::from(
        "<system-reminder>\n# Skill suggestions\nSkills relevant to your current activity \
         (invoke via the `skill` tool; full descriptions are in the system prompt's Available \
         Skills section):\n\n",
    );
    for hit in hits {
        let id = sanitize_prompt_field(hit.summary.skill_id.trim());
        let when = hint_when_to_use(hit.summary.when_to_use.as_deref());
        let reason = match &hit.reason {
            SkillMatchReason::Path { written, pattern } => {
                format!(
                    "you edited {} (matches {})",
                    sanitize_prompt_field(written),
                    sanitize_prompt_field(pattern)
                )
            }
            SkillMatchReason::Extension { written, tag } => {
                format!(
                    "you edited {} (matches tag {})",
                    sanitize_prompt_field(written),
                    sanitize_prompt_field(tag)
                )
            }
            SkillMatchReason::UserText { tag } => {
                format!("your message mentions \"{}\"", sanitize_prompt_field(tag))
            }
        };
        match when {
            Some(when) => body.push_str(&format!("- {id}: {when} — {reason}\n")),
            None => body.push_str(&format!("- {id} — {reason}\n")),
        }
    }
    body.push_str("</system-reminder>");
    body
}

/// L2 提示行的 when_to_use 展示文本：trim → 空判 → 清洗 → 截断。
/// 先清洗后截断（共享截断见 agent-types 的 truncate_scalars）：长度计数
/// 作用在已折叠/已中和的单行文本上，闭合标签不因截断而残缺
fn hint_when_to_use(when_to_use: Option<&str>) -> Option<String> {
    when_to_use
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(sanitize_prompt_field)
        .map(|text| truncate_scalars(&text, MAX_WHEN_TO_USE_HINT_SCALARS))
}

/// 采集 write pivot 输入：runtime_view 的 workspace 根（空根 = 无工作区）
pub(crate) fn workspace_root_of(runtime_view: Option<&Arc<dyn RuntimeView>>) -> Option<String> {
    let view = runtime_view?;
    let root = view.agent_context().workspace().root.clone();
    agent_types::common::workspace_root_string(&root)
}

/// `record_tool_results` 一个批次的 L2 采集产物。
#[derive(Default)]
pub(crate) struct BatchL2Signals {
    /// 本批 Completed+Success 的 skill 调用名（待 registry 解析为规范
    /// skill_id 后记账）
    pub(crate) invoked_skill_names: Vec<String>,
    /// 本批 Completed+Success 的写操作路径（write pivot 信号）
    pub(crate) written_paths: Vec<String>,
}

/// 从单个工具结果采集 L2 信号（`record_tool_results` 结果循环内逐个调用；
/// suspended 结果已在上游 `continue` 跳过——其 tool_result 由 resumer
/// 补插，不属于本批信号）。
///
/// 只认 Completed 且 Success 的结果——Denied/Failed 的调用与写操作未
/// 生效（全文未入史 / 文件未改变，不构成信号）。工具名与输入键引用
/// tool crate 导出的常量：builtin schema 演进时采集端随编译期锚点同步，
/// 不靠魔法字符串对齐。
pub(crate) fn collect_batch_l2_signal(signals: &mut BatchL2Signals, result: &ToolExecutionResult) {
    let ToolExecutionResult::Completed {
        final_call,
        raw_outcome: RawToolOutcome::Success { .. },
        ..
    } = result
    else {
        return;
    };
    match final_call.tool_name.as_str() {
        tool::SKILL_TOOL_NAME => {
            if let Some(name) = final_call
                .input
                .get(tool::SKILL_NAME_INPUT_KEY)
                .and_then(Value::as_str)
            {
                signals.invoked_skill_names.push(name.to_owned());
            }
        }
        tool::FILE_WRITE_TOOL_NAME | tool::FILE_EDIT_TOOL_NAME => {
            if let Some(path) = final_call
                .input
                .get(tool::FILE_PATH_INPUT_KEY)
                .and_then(Value::as_str)
            {
                signals.written_paths.push(path.to_owned());
            }
        }
        _ => {}
    }
}

/// 批次结算（`record_tool_results` 尾部、工具结果全部入史后调用）：
/// 1. 本批成功调用的 skill 记账——先于注入判定统一完成（全文已经
///    tool_result 进入历史，等同已加载；无论 suspended 与否、是否注入
///    都记账）。键经 registry 解析为规范 skill_id——与 SkillSummary/
///    账本过滤同一键空间，去重不依赖"调用名 == 清单 id"的隐性约定；
///    解析失败（执行侧与快照 registry 不一致的病态场景）回退原始调用名，
///    至多留下一条不匹配任何清单项的无害账目。
/// 2. write pivot 注入——suspended 协议守卫：suspended 非空时本轮不
///    注入，信号转入 `pending_write_pivot` 由恢复入口消费（守卫理由
///    见模块 doc）。
pub(crate) fn settle_batch_l2_signals(
    ctx: &mut LoopContext<'_>,
    mut signals: BatchL2Signals,
    suspended_calls: &[SuspendedToolCall],
) {
    for name in &signals.invoked_skill_names {
        let skill_id = ctx
            .snapshot
            .skill_registry
            .get_skill(name)
            .map(|spec| spec.skill_id().to_string())
            .unwrap_or_else(|| name.clone());
        ctx.state.skill_hint_sent.insert(skill_id);
    }
    if signals.written_paths.is_empty() {
        // 零成本早退红线：本批无写路径即不做任何匹配工作
        return;
    }
    if suspended_calls.is_empty() {
        let workspace_root = workspace_root_of(ctx.input.runtime_view.as_ref());
        inject_write_pivot_skill_hints(
            ctx.state,
            ctx.estimator,
            &ctx.skill_discovery,
            &signals.written_paths,
            workspace_root.as_deref(),
        );
    } else {
        ctx.state
            .pending_write_pivot
            .append(&mut signals.written_paths);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/core/skill_discovery_test.rs"]
mod tests;
