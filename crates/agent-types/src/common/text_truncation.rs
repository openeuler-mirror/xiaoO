//! 确定性文本截断（prompt 展示视图专用）。
//!
//! 单一语义来源：L1 清单 description 截断（skill crate 的
//! `FileSkillRegistry`）与 L2 提示 when_to_use 截断（core crate 的
//! `skill_discovery`）原本是两份同语义实现，收敛于此，防维护漂移。
//!
//! 截断只作用于 prompt 展示视图（system prompt 前缀不得逐轮变化的缓存
//! 红线）；管理目录 / `skill show` 等管理侧一律走 spec 访问器取全文。

/// 按 Unicode 标量截断：结果标量数不超过 `cap`，超限时保留前 `cap - 1`
/// 个标量并追加 `"…"`（省略号占一个标量、计入 cap）。
///
/// 语义契约：
/// - 按 Unicode 标量计（非字节、非字素簇）：确定性、跨平台，截断点永不
///   落在多字节标量中间，产出恒为合法 UTF-8；
/// - `cap == 0` 返回空串（结果长度永不超过 cap，省略号也不例外）；
/// - 标量数 `<= cap` 时原文逐字保留，不追加省略号；
/// - 纯函数：同一输入任意次调用产出逐字节相同的结果（prompt 前缀字节
///   稳定性由函数式保证，而非调用方约定）。
pub fn truncate_scalars(text: &str, cap: usize) -> String {
    if cap == 0 {
        return String::new();
    }
    if text.chars().count() <= cap {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(cap - 1).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-types/common/text_truncation_test.rs"]
mod tests;
