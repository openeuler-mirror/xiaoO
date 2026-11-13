//! 与具体 provider / 模型目录无关的 HTTP 层公共策略。
//!
//! 目前有三件事，都是"谁都不该自己再写一遍"的东西：
//!
//! 1. [`tls_verification_disabled`]：`NODE_TLS_REJECT_UNAUTHORIZED=0` 这个
//!    "自签名网关"开关的判据。它被 chat provider（`providers/*`）、模型目录
//!    客户端（`models/*`）以及 `memory` 的 OpenAI 兼容 embedding 客户端共用，
//!    所以放在这里并对外公开——而不是让 providers 反向依赖 `models`，或让各
//!    crate 各写一份判据。
//! 2. [`http1_forced`]：`XIAOO_LLM_FORCE_HTTP1` 这个"出站锁回 HTTP/1.1"的
//!    逃生门判据，配 [`with_forced_http1`] 用在每一个 reqwest client 构建
//!    点，避免各写一份。同一个开关也覆盖 `crates/mcp` 的出站 transport
//!    （streamable-http / SSE）：判据在 mcp 侧另有一份拷贝，免得 mcp 为
//!    三行 env 解析反向依赖 llm-client。
//! 3. [`client_build_failed`]：`reqwest::Client::builder()` 构造失败时的统一出口。

/// 是否跳过 TLS 证书校验：仅当 `NODE_TLS_REJECT_UNAUTHORIZED` 被显式设为 `0`
/// （自签名证书网关的常见做法）。
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

/// 是否把出站 client 锁回 HTTP/1.1：`XIAOO_LLM_FORCE_HTTP1` 被置为任意值
/// 时生效，"视为关闭"的取值只有三种——空串、`0`、小写 `false`（判据语义
/// 与 `XIAOO_DISABLE_OPENROUTER_LOOKUP` 一致）。没有开启值的白名单：
/// `off`、`no`、`False` 这类"看着像关闭"的值同样算开启，要关闭只允许
/// 上述三种写法或删掉变量。用途：启用 reqwest 的 `http2` feature 后，
/// 个别上游网关经 ALPN 协商到 h2 但流式实现有缺陷，现场用环境变量一键
/// 回到启用 h2 之前的 HTTP/1.1 行为，不必改代码重新发版。
pub(crate) fn http1_forced() -> bool {
    http1_forced_from(std::env::var("XIAOO_LLM_FORCE_HTTP1").ok().as_deref())
}

/// 纯函数版本：环境变量是进程全局的，测试不该去改它，因此把判据抽出来单独钉住。
/// 取值集合：`None` 与 `Some(空串 | "0" | "false")` 为关闭（仅去首尾空白，
/// 区分大小写，不做小写归一）；其余任意 `Some` 值（含 `off` / `no` / `False`）
/// 一律视为开启。
pub(crate) fn http1_forced_from(value: Option<&str>) -> bool {
    value.is_some_and(|v| !matches!(v.trim(), "" | "0" | "false"))
}

/// 把 [`http1_forced`] 应用到 reqwest builder 上：开关生效时强制
/// `http1_only()`，不生效时原样返回。供各 provider 与模型目录 client 的
/// 构建点共用，调用点不必各自判一遍。命名是"强制"而非"回退"：这是
/// 逃生开关的显式钉死，不是失败后的自动降级。
pub(crate) fn with_forced_http1(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    if http1_forced() {
        builder.http1_only()
    } else {
        builder
    }
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
    // `Display` 对 builder error 只印 "builder error"（reqwest 0.12.28
    // `error.rs:227-272`），原因在 `source` 里，只有 `Debug` 带得出来。
    tracing::error!(?error, "failed to build an HTTP client");
    panic!("failed to build an HTTP client: {error:?}")
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/http_test.rs"]
mod tests;
