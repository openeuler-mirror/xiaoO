use std::collections::BTreeSet;

use anyhow::Result;

use crate::app_state::AppState;
use crate::chat::ModelInfo;
use crate::config::{save_llm_secret, Config};
use crate::gateway::llm_assembly::{
    list_llm_models, LlmAssemblyError, LlmAssemblyInput, LlmModelSummary,
};

pub fn api_key_env_for_provider(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "openai" => "OPENAI_API_KEY",
        "anthropic" | "claude" => "ANTHROPIC_API_KEY",
        "gemini" | "google" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "groq" => "GROQ_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "xai" | "xai-grok" => "XAI_API_KEY",
        "deepseek" => "DEEPSEEK_API_KEY",
        "zai" | "zai-global" | "z.ai" | "zai-cn" | "zai-china" | "bigmodel" | "zhipu"
        | "glm-cn" => "ZHIPU_API_KEY",
        "zai-coding-plan" | "zhipu-coding-plan" | "zhipuai-coding-plan" => "ZHIPU_API_KEY",
        "glm" | "glm-global" => "GLM_API_KEY",
        "minimax"
        | "minimax-openai"
        | "minimax-anthropic"
        | "minimax-coding-plan"
        | "minimax-code-plan"
        | "minimax-token-plan" => "MINIMAX_API_KEY",
        "kimi" | "moonshot" | "moonshot-ai" => "MOONSHOT_API_KEY",
        "kimi-coding-plan" | "kimi-code-plan" | "kimi-code" | "kimi-for-coding" => "KIMI_API_KEY",
        "gitcode" => "GITCODE_API_KEY",
        "ollama" => "OLLAMA_HOST",
        "local" => "API_KEY",
        _ => "API_KEY",
    }
}

pub fn default_api_key_env_for_provider(provider: &str) -> Option<String> {
    let env_var = api_key_env_for_provider(provider);
    match env_var {
        "API_KEY" | "OLLAMA_HOST" => None,
        _ => Some(env_var.to_string()),
    }
}

pub fn default_api_base_for_provider(provider: &str) -> String {
    match provider.to_lowercase().as_str() {
        "openai" => "https://api.openai.com/v1".to_string(),
        "openrouter" => "https://openrouter.ai/api/v1".to_string(),
        "groq" => "https://api.groq.com/openai/v1".to_string(),
        "mistral" => "https://api.mistral.ai/v1".to_string(),
        "together" => "https://api.together.xyz/v1".to_string(),
        "xai" | "xai-grok" => "https://api.x.ai/v1".to_string(),
        "deepseek" => "https://api.deepseek.com/v1".to_string(),
        "gitcode" => "https://api-ai.gitcode.com/v1".to_string(),
        "minimax" | "minimax-openai" => "https://api.minimaxi.com/v1".to_string(),
        "minimax-anthropic" => "https://api.minimaxi.com/anthropic/v1".to_string(),
        "minimax-coding-plan" | "minimax-code-plan" | "minimax-token-plan" => {
            "https://api.minimax.io/v1".to_string()
        }
        "kimi" | "moonshot" | "moonshot-ai" => "https://api.moonshot.cn/v1".to_string(),
        "kimi-coding-plan" | "kimi-code-plan" | "kimi-code" | "kimi-for-coding" => {
            "https://api.kimi.com/coding/v1".to_string()
        }
        "ollama" => "http://localhost:11434".to_string(),
        "local" => "http://localhost:8080/v1".to_string(),
        "zai-coding-plan" | "zhipu-coding-plan" | "zhipuai-coding-plan" => {
            "https://api.z.ai/api/coding/paas/v4".to_string()
        }
        _ => String::new(),
    }
}

pub fn persisted_selection_settings(config: &Config, provider: &str) -> (Option<String>, String) {
    let default_api_key_env = default_api_key_env_for_provider(provider);
    let default_api_base = default_api_base_for_provider(provider);

    if config.llm.provider.eq_ignore_ascii_case(provider) {
        let api_key_env = config.llm.api_key_env.clone().or(default_api_key_env);
        let api_base = if config.llm.api_base.trim().is_empty() {
            default_api_base
        } else {
            config.llm.api_base.clone()
        };
        (api_key_env, api_base)
    } else {
        (default_api_key_env, default_api_base)
    }
}

pub fn persist_active_provider_selection(
    state: &mut AppState,
    provider: String,
    model: String,
    api_key_env: Option<String>,
    api_base: String,
) {
    persist_active_provider_selection_internal(state, provider, model, api_key_env, api_base);
}

fn persist_active_provider_selection_internal(
    state: &mut AppState,
    provider: String,
    model: String,
    api_key_env: Option<String>,
    api_base: String,
) {
    let mut cfg = Config::load_from(&state.config_path).unwrap_or_default();
    cfg.llm.provider = provider.clone();
    cfg.llm.model = model.clone();
    cfg.llm.api_key_env = api_key_env.clone();
    cfg.llm.api_base = api_base.clone();
    if let Err(error) = cfg.save_to(&state.config_path) {
        tracing::warn!("Failed to save config: {}", error);
    }

    state.agent_config.llm.provider = provider.clone();
    state.agent_config.llm.model = model.clone();
    state.agent_config.llm.api_key_env = api_key_env;
    state.agent_config.llm.api_base = api_base;

    state.status_panel.set_provider(&provider, &model);
}

pub fn validate_and_connect_api_key(
    state: &mut AppState,
    provider: String,
    model: String,
    api_key: &str,
) -> Result<(), String> {
    let env_var = api_key_env_for_provider(&provider).to_string();
    let api_base = default_api_base_for_provider(&provider);
    std::env::set_var(&env_var, api_key);

    if let Err(error) = save_llm_secret(&state.config_path, &env_var, api_key) {
        tracing::warn!("Failed to save API key: {}", error);
    }

    persist_active_provider_selection_internal(state, provider, model, Some(env_var), api_base);
    Ok(())
}

/// How a copy request was delivered to the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOutcome {
    /// The system clipboard was set directly via a native tool
    /// (wl-copy / xclip / xsel / arboard). Delivery is confirmed.
    Native,
    /// An OSC 52 escape sequence was emitted to the terminal. Delivery is
    /// fire-and-forget: there is no acknowledgement, so the terminal may
    /// silently ignore it (PuTTY, MobaXterm, FinalShell, Xshell and
    /// VTE-based terminals do not implement OSC 52 by default).
    Osc52,
}

impl ClipboardOutcome {
    /// Whether the copy is confirmed — or at least very likely — to have
    /// reached the clipboard the user will paste from. Only [`Self::Native`]
    /// is confirmed; [`Self::Osc52`] additionally requires a terminal known
    /// to implement the sequence (see [`osc52_likely_supported`]).
    pub fn delivered(&self) -> bool {
        match self {
            ClipboardOutcome::Native => true,
            ClipboardOutcome::Osc52 => osc52_likely_supported(),
        }
    }
}

/// Best-effort check whether the current terminal is *known* to implement
/// the OSC 52 clipboard sequence. OSC 52 has no acknowledgement mechanism,
/// so this sniffs `TERM` / `TERM_PROGRAM` / well-known terminal env vars.
///
/// Returns `false` for unknown terminals — PuTTY, MobaXterm, FinalShell and
/// friends all report themselves as plain `xterm`/`xterm-256color`, VTE and
/// stock xterm disable the feature by default — which is the safe answer:
/// the UI then reports a copy failure instead of claiming success while the
/// clipboard silently kept its old contents.
///
/// Inside tmux this stays conservative: tmux masks the pane `TERM` with its
/// own `default-terminal` (`screen*`/`tmux*`), so the outer terminal can
/// only be identified through signals that survive the tmux session
/// environment snapshot (`TERM_PROGRAM`, `WT_SESSION`, `KITTY_WINDOW_ID`).
/// `TMUX` alone proves nothing — with `allow-passthrough off` (the tmux >=
/// 3.3 default) the DCS passthrough is silently dropped, and even when the
/// sequence does reach the outer terminal it may not implement OSC 52.
/// The snapshot can go stale when a session is created in one terminal and
/// later attached from another; that rare case is accepted rather than
/// queried.
pub fn osc52_likely_supported() -> bool {
    osc52_supported_by_terminal(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var_os("WT_SESSION").is_some(),
        std::env::var_os("KITTY_WINDOW_ID").is_some(),
    )
}

/// Pure form of [`osc52_likely_supported`] so the heuristic stays testable
/// without mutating process-global env vars.
fn osc52_supported_by_terminal(
    term: Option<&str>,
    term_program: Option<&str>,
    wt_session: bool,
    kitty_window_id: bool,
) -> bool {
    // Windows Terminal sets WT_SESSION and implements OSC 52. The variable
    // is not in tmux's default `update-environment` list, so it survives
    // into panes as part of the session environment snapshot.
    if wt_session {
        return true;
    }
    // kitty refuses to set TERM_PROGRAM (kovidgoyal/kitty#3317) and is only
    // recognizable through its own env vars; KITTY_WINDOW_ID is set per
    // window and survives into tmux panes the same way. Without this, a
    // kitty + tmux setup would be reported as unconfirmed even though
    // kitty implements OSC 52.
    if kitty_window_id {
        return true;
    }
    // Terminals that identify themselves through `TERM` (kitty, Alacritty,
    // WezTerm, ghostty, foot, rio, contour, mintty). `TERM`-suffix variants
    // such as `alacritty-direct` / `foot-extra` are covered by the prefix
    // match below. Inside tmux the pane `TERM` is `screen*`/`tmux*` and
    // never matches; a hand-configured `default-terminal xterm-kitty`
    // counts as an explicit opt-in.
    const KNOWN_TERMS: [&str; 9] = [
        "xterm-kitty",
        "alacritty",
        "wezterm",
        "xterm-ghostty",
        "ghostty",
        "foot",
        "rio",
        "contour",
        "mintty",
    ];
    if let Some(term) = term {
        if KNOWN_TERMS
            .iter()
            .any(|known| term == *known || term.starts_with(&format!("{known}-")))
        {
            return true;
        }
    }
    // Terminals that identify themselves through `TERM_PROGRAM` (VS Code,
    // iTerm2, WezTerm, ghostty, mintty, Hyper, Tabby, Warp). Inside tmux
    // this is the main surviving signal: TERM_PROGRAM is not refreshed by
    // tmux's `update-environment`, so the outer terminal's value is
    // inherited by every pane.
    const KNOWN_PROGRAMS: [&str; 8] = [
        "vscode",
        "iTerm.app",
        "WezTerm",
        "ghostty",
        "mintty",
        "Hyper",
        "Tabby",
        "WarpTerminal",
    ];
    term_program.is_some_and(|program| KNOWN_PROGRAMS.contains(&program))
}

pub fn copy_to_clipboard(text: &str) -> Result<ClipboardOutcome> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    // Wayland: wl-copy
    if std::env::var("WAYLAND_DISPLAY").is_ok()
        && Command::new("wl-copy")
            .arg(text)
            .output()
            .ok()
            .map(|o| o.status.success())
            == Some(true)
        {
            return Ok(ClipboardOutcome::Native);
        }
    // X11: xclip (only if DISPLAY is set)
    if std::env::var("DISPLAY").is_ok() {
        if let Ok(mut child) = Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            if child.wait().ok().map(|s| s.success()) == Some(true) {
                return Ok(ClipboardOutcome::Native);
            }
        }
        // X11: xsel
        if let Ok(mut child) = Command::new("xsel")
            .args(["--clipboard", "--input"])
            .stdin(Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            if child.wait().ok().map(|s| s.success()) == Some(true) {
                return Ok(ClipboardOutcome::Native);
            }
        }
        // Fallback: arboard (only makes sense with a display server)
        if let Ok(mut clip) = arboard::Clipboard::new() {
            if clip.set_text(text).is_ok() {
                return Ok(ClipboardOutcome::Native);
            }
        }
    }

    // OSC 52 fallback: works in most modern terminals including Windows
    // Terminal, iTerm2, kitty, Alacritty, and over SSH. There is no
    // acknowledgement mechanism, so delivery is *not* confirmed — callers
    // must check `ClipboardOutcome::delivered()` before claiming success,
    // otherwise PuTTY/MobaXterm/FinalShell-style terminals (which ignore the
    // sequence) keep the previous clipboard contents while the UI had shown
    // "Copied to clipboard".
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Write directly to stdout while still in raw mode.
    let mut out = std::io::stdout().lock();
    // When running inside tmux, emit BOTH delivery paths: tmux gates each
    // behind a separate option and its defaults enable neither.
    //   - The raw sequence: tmux intercepts it and forwards to the outer
    //     terminal (and its own paste buffer) when `set-clipboard` is `on`.
    //   - The DCS `tmux;` passthrough wrap: bypasses tmux and reaches the
    //     outer terminal directly when `allow-passthrough` is `on` (the
    //     tmux >= 3.3 default is off, which silently drops the wrap — the
    //     trap opencode fell into, see anomalyco/opencode#19982).
    // When both options are on the outer terminal receives the same payload
    // twice, which is harmless; when neither is on nothing is delivered and
    // `delivered()` already reports tmux copies as unconfirmed unless a
    // known outer terminal was detected.
    if std::env::var("TMUX").is_ok() {
        write!(out, "\x1b]52;c;{encoded}\x07")?;
        write!(out, "\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\")?;
    } else {
        write!(out, "\x1b]52;c;{encoded}\x07")?;
    }
    out.flush()?;
    Ok(ClipboardOutcome::Osc52)
}

// ---------------------------------------------------------------------------
// 官方 model list 拉取
// ---------------------------------------------------------------------------

/// 一次官方 model list 拉取所需的全部参数（纯数据，从 TUI 配置推导）。
#[derive(Debug, Clone)]
pub struct ProviderModelFetchRequest {
    pub provider: String,
    pub api_base: String,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
}

/// 拉取失败的分类：UI 需要区分"只是没配 key"这类提示与真正的失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFetchError {
    /// 缺少环境变量（变量名）。属于配置提示，按 muted 渲染而非错误色。
    MissingApiKey(String),
    /// provider profile 声明 `supports_model_catalog = false`：不发请求、不重试。
    Unsupported,
    /// 其它失败：网络、HTTP 状态、解析、超时、base_url 缺失等。
    Failed(String),
}

/// 拉取结果。`provider` 随结果一起回传，调用方按名字应用——不依赖下标，
/// 因为弹窗可能已经关闭，或用户已经切到别的 provider。
#[derive(Debug, Clone)]
pub struct ProviderModelFetchOutcome {
    pub provider: String,
    pub result: Result<Vec<ModelInfo>, ModelFetchError>,
}

/// 由 TUI 配置推导拉取参数。
///
/// 复用 `persisted_selection_settings`：目标 provider 就是当前激活 provider 时
/// 用配置里的 `api_base` / `api_key_env`（空串回落到默认表），否则用该 provider
/// 的默认值——与用户在该 provider 上按 Enter 时应用的设置完全一致。
///
/// `api_key` 从 `api_key_env` 指向的环境变量读取：TUI 启动时已调用
/// `inject_llm_secrets_into_env`，API key 弹窗也会 `set_var`，因此 secret store
/// 里保存的 key 能被读到。
pub fn model_fetch_request(config: &Config, provider: &str) -> ProviderModelFetchRequest {
    let (api_key_env, api_base) = persisted_selection_settings(config, provider);
    let api_key = api_key_env
        .as_deref()
        .and_then(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    ProviderModelFetchRequest {
        provider: provider.to_string(),
        api_base,
        api_key_env,
        api_key,
    }
}

/// 调官方 model list 接口。
///
/// 经门面 `list_llm_models`（按 protocol family 分派到 OpenAI 兼容 / Anthropic /
/// Gemini / Ollama / Zhipu），endside 不直接依赖 `llm-client`、也不自己发 HTTP。
///
/// 本函数永不返回 `Err`：任何失败都收敛成 [`ProviderModelFetchOutcome`] 里的
/// `Err`，这样每个 spawn 出去的拉取任务必定回传一个结果，调用方的 `Loading`
/// 状态不会永远挂着。
/// 等待拉取任务结束，并把"任务没能正常返回"（panic / 被取消）也转成一个失败结果。
///
/// 任务回传的结果是**唯一**能把状态从 `Loading` 推走的输入（见
/// `App::apply_model_fetch_outcome`）。若任务 panic 而没有回传，该 provider 会永久停在
/// `Loading`：`needs_refresh(Loading)` 为假，`start_model_fetch` 的守卫会拦住所有重试，
/// 并且它还会永久占着在飞名额。
/// 所以这里保证：无论任务以何种方式结束，都必须有一个结果回传给 UI。
pub async fn join_fetch_outcome(
    provider: String,
    handle: tokio::task::JoinHandle<ProviderModelFetchOutcome>,
) -> ProviderModelFetchOutcome {
    match handle.await {
        Ok(outcome) => outcome,
        Err(error) => {
            let reason = if error.is_panic() {
                "model list task panicked"
            } else {
                "model list task was cancelled"
            };
            tracing::warn!(provider, reason, "model list fetch task did not return");
            ProviderModelFetchOutcome {
                provider,
                result: Err(ModelFetchError::Failed(reason.to_string())),
            }
        }
    }
}

pub async fn fetch_provider_models(
    request: ProviderModelFetchRequest,
) -> ProviderModelFetchOutcome {
    let ProviderModelFetchRequest {
        provider,
        api_base,
        api_key_env,
        api_key,
    } = request;
    // profile 静态声明没有官方接口时直接短路：门面会**先**解析 API key 再判断
    // catalog 支持，若不前置，缺 key 的无 catalog provider（minimax / gitcode）
    // 会得到"需要配置 XXX_API_KEY"——用户配了 key 也依然拉不到，属误导。
    if catalog_declared_unsupported(&provider) {
        return ProviderModelFetchOutcome {
            provider,
            result: Err(ModelFetchError::Unsupported),
        };
    }
    let result = list_llm_models(LlmAssemblyInput {
        provider: provider.clone(),
        model: String::new(),
        api_base: non_empty(api_base),
        api_key,
        api_key_env,
        context_window_override: None,
        agent_id: None,
    })
    .await
    .map(model_infos_from_summaries)
    .map_err(map_model_fetch_error);
    // 失败必须留痕：这条路径过去既没有日志、UI 也没有提示，等于静默降级回内置列表。
    // 文案走与屏幕相同的消毒（key 不会进日志）。
    match &result {
        Err(ModelFetchError::Failed(message)) => {
            tracing::warn!(provider = %provider, error = %message, "model list fetch failed");
        }
        Err(ModelFetchError::MissingApiKey(env)) => {
            tracing::info!(provider = %provider, env, "model list fetch skipped: API key not set");
        }
        _ => {}
    }
    ProviderModelFetchOutcome { provider, result }
}

/// provider profile 是否明确声明"没有官方模型列表接口"。
///
/// 未知 provider（registry 里没有）返回 `false`：那种情况交给门面报
/// `unknown provider: …`，比这里伪造一个 `Unsupported` 更有信息量。
fn catalog_declared_unsupported(provider: &str) -> bool {
    xiaoo_api::llm::resolve_provider_profile(provider)
        .map(|profile| !profile.supports_model_catalog)
        .unwrap_or(false)
}

/// 外部端点文本的显示上限：模型 id / 错误文案都该是一句话，超长只可能是异常
/// 返回或攻击载荷。
const MAX_EXTERNAL_TEXT_CHARS: usize = 200;

/// 单个 provider 最多接受的模型条数。官方目录的正常量级是几十到几百条
/// （openrouter 300+），几万条只可能是异常端点或攻击载荷——而列表每帧都要建行，
/// 必须有上限兜住渲染与内存。
const MAX_MODELS_PER_PROVIDER: usize = 2_000;

/// 会被终端"吃掉"或用于视觉欺骗的不可见字符。`char::is_control()` 只覆盖 Cc，而
/// Cf（格式字符）/Zl/Zp 能穿过 ratatui 的字素过滤：bidi 覆盖（U+202E）可以把模型名
/// 渲染成完全不同的字符顺序，零宽字符能让两个不同的名字看起来一模一样。
fn is_invisible_or_directional(ch: char) -> bool {
    matches!(ch,
        '\u{00AD}'                    // 软连字符
        | '\u{061C}'                  // 阿拉伯字母标记
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'    // 零宽 + bidi 标记
        | '\u{2028}' | '\u{2029}'    // 行 / 段分隔符
        | '\u{202A}'..='\u{202E}'    // bidi 嵌入 / 覆盖
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}'    // bidi 隔离
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}' // tag 字符
    )
}

/// URL 只保留 `scheme://host`，路径 / query / fragment 一律抹掉。
///
/// 抹 query 是因为 provider 的 key 可能在 query 里（gemini 的 `?key=`）；保留主机名
/// 是因为"哪个端点失败"是排查 TLS / 网关问题的关键信息——全抹成 `<url>` 会让自签名
/// 网关这类问题无从下手。
fn redact_url(token: &str) -> String {
    let Some((scheme, rest)) = token.split_once("://") else {
        return "<redacted>".to_string();
    };
    let host_end = rest.find('/').unwrap_or(rest.len());
    format!("{scheme}://{}/<redacted>", &rest[..host_end])
}

/// 形如 `key=…` / `api_key: …` / `token=…` 的字段（大小写不敏感）。
fn looks_like_secret_field(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    [
        "key=", "key:", "token=", "token:", "secret=", "secret:", "password=", "password:",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// 外部端点返回的**标签类**文本（模型 id、显示名、环境变量名）进入 TUI 前的消毒：
/// 这些字符串会被渲染到弹窗（`overlay.rs`）、状态行与会话消息里，恶意或异常端点
/// 可以借此注入控制序列（经典 TUI 注入面：`\x1b` 能清屏、改标题、伪造按键），
/// 因此只保留可打印字符，并限制长度。
fn sanitize_external_label(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control() && !is_invisible_or_directional(*ch))
        .take(MAX_EXTERNAL_TEXT_CHARS)
        .collect()
}

/// 外部端点返回的**说明类**文本（错误文案）消毒：除控制字符外还要抹掉 URL——底层
/// 客户端的错误串会带请求 URL，而 provider 的 URL query 可能含 API key（例如
/// gemini 的 `?key=`），这段文案是要显示在屏幕上的。
fn sanitize_external_message(raw: &str) -> String {
    // 控制字符与不可见字符换成空格而不是删掉：`...\nkey=xxx` 若直接删掉换行，会粘成
    // 一个新 token，下面的 token 规则就失效了。
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_control() || is_invisible_or_directional(ch) {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let mut redact_next = false;
    cleaned
        .split_whitespace()
        .map(|token| {
            // 顺序重要：带 scheme 的整串先按 URL 处理（保留"在哪失败"的信息），再看
            // 是否是裸露的密钥字段。
            if token.contains("://") {
                redact_url(token)
            } else if redact_next {
                redact_next = false;
                "<redacted>".to_string()
            } else if looks_like_secret_field(token) {
                "<redacted>".to_string()
            } else if token.eq_ignore_ascii_case("bearer") || token.eq_ignore_ascii_case("basic") {
                redact_next = true;
                token.to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_EXTERNAL_TEXT_CHARS)
        .collect()
}

/// `LlmModelSummary` → TUI 的 `ModelInfo`：显示名优先官方 `display_name`，
/// 按 id 去重并跳过空 id，顺序保持官方返回顺序（不排序、不加工）。
///
/// id 与显示名来自外部端点，先经 [`sanitize_external_label`] 消毒再进入渲染层。
pub fn model_infos_from_summaries(summaries: Vec<LlmModelSummary>) -> Vec<ModelInfo> {
    let mut seen = BTreeSet::new();
    summaries
        .into_iter()
        .filter_map(|summary| {
            let id = sanitize_external_label(summary.id.trim());
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let name = summary
                .display_name
                .map(|name| sanitize_external_label(name.trim()))
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            Some(ModelInfo { id, name })
        })
        .take(MAX_MODELS_PER_PROVIDER)
        .collect()
}

fn map_model_fetch_error(error: LlmAssemblyError) -> ModelFetchError {
    match error {
        LlmAssemblyError::MissingApiKeyEnv(name) => {
            ModelFetchError::MissingApiKey(sanitize_external_label(&name))
        }
        LlmAssemblyError::ModelCatalogUnsupported(_) => ModelFetchError::Unsupported,
        other => ModelFetchError::Failed(sanitize_external_message(&other.to_string())),
    }
}

/// 空串 / 全空白 → `None`，让门面回落到 provider profile 的默认 base_url。
fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/services/provider_test.rs"]
mod tests;
