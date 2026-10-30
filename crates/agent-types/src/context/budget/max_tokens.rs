//! `max_tokens` 与上下文窗口关系的共用校验策略。
//!
//! 两个执行点共用本模块，保证"何时判定配置不可用"与用户文案只有一份真相：
//! - TUI 轮次分发（`apps/endside/src/gateway_api/runtime_request.rs` 的
//!   `evaluate_max_tokens_against_window`）：只对探测确认的窗口调用，失败拒绝单轮；
//! - daemon 启动与运行时热切换（`apps/serverside/src/daemon_runtime.rs` 的
//!   `validate_token_budget_config`）：对装配解析出的窗口调用，失败 abort 进程。
//!
//! 窗口的信任语义（探测值 / 静态估算）由调用方决定——本模块只拿到一个已解析
//! 的 `usize` 窗口，因此判断与文案不掺杂信任策略；拒绝与 abort 的处置差异同
//! 样留在调用方。

use std::path::Path;

/// 输出预留占窗口的比例上限：超过即认为输出过度挤占输入空间，daemon 侧据此
/// 打告警。TUI 轮次路径不设比例告警——其窗口只有可信/估算两级语义，比例阈值
/// 落在估算窗口上会产生噪音，故仅做"输入空间为零"的硬拒。常量与校验谓词
/// 同处一地，阈值与判定语义可对照，防止各自漂移。
pub const MAX_REASONABLE_OUTPUT_RATIO: f64 = 0.5;

/// 判定与文案的单一真相源：`max_tokens >= window` 意味着输入空间为零，配置
/// 不可用。`Err` 携带面向用户的统一文案；`config_section` 由调用方传入，因为
/// 两端配置 schema 不同（TUI 的 `[llm]` 与 daemon 的 `[app.llm]`），其余字段
/// 两端一致，不允许各自改写。
pub fn validate_max_tokens_against_window(
    max_tokens: usize,
    window: usize,
    model: &str,
    config_path: &Path,
    config_section: &str,
) -> Result<(), String> {
    if max_tokens < window {
        return Ok(());
    }
    Err(format!(
        "max_tokens {} is too large relative to the context window {} for model {}. \
         This would leave NO space for input.\n\
         Configuration file: {}\n\
         Suggestions:\n\
           - Reduce max_tokens (currently {}) in {} section",
        max_tokens,
        window,
        model,
        config_path.display(),
        max_tokens,
        config_section,
    ))
}
