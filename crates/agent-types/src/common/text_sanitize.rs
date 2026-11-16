//! prompt 展示视图专用的最小文本清洗（与 `text_truncation` 同属确定性
//! 纯函数，单一语义来源）。
//!
//! 收敛动机：第三方可写的 skill 元数据（skill_id / description /
//! when_to_use / paths glob / tags）进入 prompt 前的插值清洗原本散落在
//! L2 发现提示（core crate 的 `skill_discovery`）与 L1 清单段（prompt
//! crate 的 `compose_skill_section`），两份同语义实现易漂移，统一于此。
//!
//! 清洗只作用于 prompt 展示文本：账本键、`/skill-name` 展开等逻辑路径
//! 仍使用原始元数据（逐字一致），不受影响。

/// skill 元数据进入 prompt 展示文本前的最小清洗（确定性、与来源无关）：
/// - 换行/回车折叠为空格：防伪结构（标题、列表、空行伪造消息/段落边界）
/// - 中和闭合标签：`</system-reminder`（ASCII 大小写不敏感）剥掉斜杠，
///   变成 `<system-reminder` 后不再是合法闭合形式，无法逃逸包裹层
pub fn sanitize_prompt_field(raw: &str) -> String {
    neutralize_reminder_close(&raw.replace(['\r', '\n'], " "))
}

/// 大小写不敏感地把 `</system-reminder` 剥斜杠为 `<system-reminder`。
/// 匹配到标签名即中和（不要求尾随 `>`），连带覆盖 `</system-reminder >`
/// 等宽松写法；正文其余部分逐字保留（含合法的尖括号/代码片段）。
/// `</system-reminder` 为纯 ASCII，字节级比较不会切进多字节标量内部
fn neutralize_reminder_close(text: &str) -> String {
    const CLOSE_TAG: &str = "</system-reminder";
    let lowered = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if lowered[index..].starts_with(CLOSE_TAG) {
            out.push_str("<system-reminder");
            index += CLOSE_TAG.len();
        } else {
            let ch = text[index..]
                .chars()
                .next()
                .expect("index at char boundary");
            out.push(ch);
            index += ch.len_utf8();
        }
    }
    out
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-types/common/text_sanitize_test.rs"]
mod tests;
