//! 与具体 provider / 模型目录无关的 HTTP 层公共策略。
//!
//! 目前有两件事，都是"谁都不该自己再写一遍"的东西：
//!
//! 1. `NODE_TLS_REJECT_UNAUTHORIZED=0` 这个"自签名网关"开关的判据。它被 chat
//!    provider（`providers/*`）、模型目录客户端（`models/*`）以及 `memory` 的
//!    OpenAI 兼容 embedding 客户端共用，所以放在这里并对外公开——而不是让
//!    providers 反向依赖 `models`，或让各 crate 各写一份判据。
//! 2. [`client_build_failed`]：`reqwest::Client::builder()` 构造失败时的统一出口
//!    （记日志，再带着原因 panic）。

/// 是否跳过 TLS 证书校验：仅当 `NODE_TLS_REJECT_UNAUTHORIZED` 被显式设为 `0`
/// （自签名证书网关的常见做法）。chat provider、模型目录客户端与 embedding
/// 客户端共用这一个判据。
pub fn tls_verification_disabled() -> bool {
    tls_verification_disabled_from(
        std::env::var("NODE_TLS_REJECT_UNAUTHORIZED")
            .ok()
            .as_deref(),
    )
}

/// 纯函数版本：环境变量是进程全局的，测试不该去改它，因此把判据抽出来单独钉住。
pub(crate) fn tls_verification_disabled_from(value: Option<&str>) -> bool {
    value.map(|v| v.trim() == "0").unwrap_or(false)
}

/// `reqwest::Client::builder()` 构造失败时的统一出口：先记日志，再带着原因 panic。
///
/// 这里没有降级可做——`reqwest::Client::new()` 内部就是
/// `build().expect("Client::new()")`（reqwest 0.12.28 `client.rs:2482`），构建期
/// 失败会立刻再 panic 一次，却只留下一条 `Client::new(): <err>`：panic 位置落在
/// reqwest 内部，看不出是哪个调用点，也没有任何日志。构建期失败不是理论上的——
/// native-roots 就在 `build()` 里逐个 `RootCertStore::add`（`client.rs:702-731`），
/// 加载到的证书全部被拒（成功 0、失败 > 0）时直接返回 Err。仓库没有自定义 panic
/// hook，TUI 下 stderr 上的 panic 往往看不见，所以日志是唯一能留存的线索。
///
/// 返回类型写成 `reqwest::Client` 而非 `!`，只为能直接当 `Result::unwrap_or_else`
/// 的 handler：`!` 不会在泛型位置做 Never 强转，`-> !` 会被 `unwrap_or_else` 拒绝。
/// 函数体永远 panic，不返回。
pub fn client_build_failed(error: reqwest::Error) -> reqwest::Client {
    // `Display` 对 builder error 只印 "builder error"（reqwest `error.rs:227-272`），
    // 原因在 `source` 里，只有 `Debug` 带得出来。
    tracing::error!(?error, "failed to build an HTTP client");
    panic!("failed to build an HTTP client: {error:?}")
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/http_test.rs"]
mod tests;
