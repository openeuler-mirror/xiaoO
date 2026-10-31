//! `max_tokens` 与上下文窗口关系的共用校验策略。
//!
//! 两个执行点共用本模块，保证"何时判定配置不可用"与用户文案只有一份真相：
//! - TUI 轮次分发（`apps/endside/src/gateway_api/runtime_request.rs` 的
//!   `evaluate_max_tokens_against_window`）：对两种信任级别的窗口都调用，
//!   失败拒绝单轮；
//! - daemon 启动与运行时热切换（`apps/serverside/src/daemon_runtime.rs` 的
//!   `validate_token_budget_config`）：对装配解析出的窗口调用，失败 abort 进程。
//!
//! 系统预留的计算（[`system_reserve`]）同样定档于此：TUI 的拒绝线与 daemon
//! 的预算装配共用同一算法。但两端处置不同：TUI 对输出与预留吃满窗口的配置
//! 硬拒单轮，daemon 在 `build_token_budget` 钳制输出后照常运行——同一配置
//! 可能在 TUI 被拒、在 daemon 降级运行，两端的拒绝集并不重合。
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

/// 系统提示预留的默认 token 数，经 [`system_reserve`] 生效;直接引用该常量
/// 自行取 `min` 的写法一律视为绕过单一真相源。
pub const DEFAULT_SYSTEM_TOKEN_RESERVE: usize = 2048;

/// 从窗口中划出的系统提示预留。`min` 与 `saturating_sub(1)` 合起来保证
/// reserve 严格小于窗口(窗口 ≤1 时归零)，daemon 侧 `build_token_budget`
/// 的连环 `saturating_sub` 因此至少给输出留 1 token。该上界并不让小窗口
/// 变得可用——系统提示的真实占用不随窗口缩小，窗口不超过
/// `DEFAULT_SYSTEM_TOKEN_RESERVE` + 1 时，TUI 对任何正 `max_tokens`
/// 一律硬拒单轮并报“窗口过小”（见 [`validate_max_tokens_against_window`]），
/// 属预期行为。TUI 的 `evaluate_max_tokens_against_window` 与 daemon 的
/// `build_token_budget` 共用本函数。
pub fn system_reserve(window: usize) -> usize {
    DEFAULT_SYSTEM_TOKEN_RESERVE.min(window.saturating_sub(1))
}

/// 判定与文案的单一真相源：`max_tokens + system_reserve >= window` 意味着
/// 输入空间为零，配置不可用。`system_reserve` 为窗口外必须保底的 token 数
/// （TUI 传系统预留；daemon 传 0，其输出预留已在 `build_token_budget` 钳制）。
/// 加法饱和而非回绕：daemon 的 `max_tokens` 是 usize 配置，极端大值回绕变小
/// 会错误地绕过 abort 线。非零 reserve 必须在文案中展示数值，否则
/// "max_tokens 小于窗口却被拒"对用户是黑箱；0 是钳制标志而非真实预留，
/// 照实展示反而误导，故分句随 0 省略。
/// `config_section` 由调用方传入（TUI 的 `[llm]` 与 daemon 的 `[app.llm]`）。
pub fn validate_max_tokens_against_window(
    max_tokens: usize,
    window: usize,
    system_reserve: usize,
    model: &str,
    config_path: &Path,
    config_section: &str,
) -> Result<(), String> {
    if max_tokens.saturating_add(system_reserve) < window {
        return Ok(());
    }
    let reserve_clause = if system_reserve == 0 {
        String::new()
    } else {
        format!(
            "together with the system reserve of {} tokens, ",
            system_reserve
        )
    };
    // 窗口扣除预留后连 1 token 输出都容不下时，唯一能过门的 max_tokens 是 0，
    // 而两端配置层都禁止 0——此时"下调 max_tokens"是死胡同建议，改为直指
    // 窗口过小。
    if window.saturating_sub(system_reserve) <= 1 {
        return Err(format!(
            "the context window {} for model {} is too small: {}no positive max_tokens \
             value leaves space for input.\n\
             Configuration file: {}\n\
             Suggestions:\n\
               - Use a model with a larger context window",
            window,
            model,
            reserve_clause,
            config_path.display(),
        ));
    }
    Err(format!(
        "max_tokens {} is too large relative to the context window {} for model {}: \
         {}this would leave NO space for input.\n\
         Configuration file: {}\n\
         Suggestions:\n\
           - Reduce max_tokens (currently {}) in {} section",
        max_tokens,
        window,
        model,
        reserve_clause,
        config_path.display(),
        max_tokens,
        config_section,
    ))
}
