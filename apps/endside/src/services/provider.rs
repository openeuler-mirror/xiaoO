use std::collections::BTreeSet;

use anyhow::Result;
use std::sync::Arc;

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

    let agent_config = Arc::make_mut(&mut state.agent_config);
    agent_config.llm.provider = provider.clone();
    agent_config.llm.model = model.clone();
    agent_config.llm.api_key_env = api_key_env;
    agent_config.llm.api_base = api_base;

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
    /// fire-and-forget: terminals are free to silently ignore the sequence
    /// (which ones implement it — and which report themselves misleadingly —
    /// see [`osc52_likely_supported`]).
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
    // Terminal, iTerm2, kitty, Alacritty, and over SSH. Delivery is
    // fire-and-forget — callers must check `ClipboardOutcome::delivered()`
    // before claiming success; the terminal blacklist and detection
    // heuristic live in [`osc52_likely_supported`].
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Write directly to stdout while still in raw mode.
    let mut out = std::io::stdout().lock();
    // When running inside tmux, emit BOTH delivery paths: tmux gates each
    // behind a separate option and its defaults enable neither.
    //   - The raw sequence: tmux intercepts it and forwards to the outer
    //     terminal (and its own paste buffer) when `set-clipboard` is `on`.
    //   - The DCS `tmux;` passthrough wrap: bypasses tmux and reaches the
    //     outer terminal directly when `allow-passthrough` is `on` (off by
    //     default since tmux 3.3, silently dropping the wrap — see
    //     anomalyco/opencode#19982; detection-side implications are covered
    //     in [`osc52_likely_supported`]).
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
// Official model-list fetching
// ---------------------------------------------------------------------------

/// Everything one official model-list fetch needs (plain data, derived
/// from the TUI config).
#[derive(Debug, Clone)]
pub struct ProviderModelFetchRequest {
    pub provider: String,
    pub api_base: String,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
}

/// Failure classification: the UI needs to tell hints like "no key
/// configured" apart from real failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFetchError {
    /// Missing environment variable (its name). A configuration hint,
    /// rendered muted instead of as an error.
    MissingApiKey(String),
    /// The provider profile declares `supports_model_catalog = false`: no
    /// request, no retry.
    Unsupported,
    /// Any other failure: network, HTTP status, parsing, timeout, missing
    /// base_url, …
    Failed(String),
}

/// Fetch outcome. The `provider` travels with the result and the caller
/// applies it by name — never by index, since the dialog may already be
/// closed or the user may have switched providers.
#[derive(Debug, Clone)]
pub struct ProviderModelFetchOutcome {
    pub provider: String,
    pub result: Result<Vec<ModelInfo>, ModelFetchError>,
}

/// Derive fetch parameters from the TUI config.
///
/// Reuses `persisted_selection_settings`: when the target provider is the
/// active one, the config's `api_base` / `api_key_env` apply (empty strings
/// fall back to the default table), otherwise that provider's defaults —
/// exactly the settings applied when the user presses Enter on it.
///
/// `api_key` is read from the environment variable named by `api_key_env`:
/// the TUI already called `inject_llm_secrets_into_env` at startup and the
/// API-key dialog also `set_var`s, so keys stored in the secret store are
/// visible here.
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

/// Await the fetch task and turn "the task never returned normally"
/// (panic / cancellation) into a failure outcome too.
///
/// The task's report-back is the preferred input for leaving `Loading` (see
/// `App::apply_model_fetch_outcome`). If the task vanishes without
/// reporting: the in-flight slot stays taken, force cannot bypass
/// `begin_model_fetch`'s dedup and global cap, and the provider must wait
/// out the full `MODEL_LIST_STALE_AFTER` backstop window before a re-fetch
/// (state-machine details in `ChatState::needs_refresh` and
/// `App::start_model_fetch`). Hence the guarantee here: however the task
/// ends, one outcome must reach the UI so the user never waits in vain.
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

/// Call the official model-list endpoint.
///
/// Goes through the `list_llm_models` facade (dispatched per protocol
/// family to OpenAI-compatible / Anthropic / Gemini / Ollama / Zhipu);
/// endside neither depends on `llm-client` directly nor issues its own HTTP.
///
/// Never returns `Err`: every failure collapses into the `Err` inside
/// [`ProviderModelFetchOutcome`], so every spawned fetch task is guaranteed
/// to report one outcome and the caller's `Loading` state can never hang
/// forever (backstop: `join_fetch_outcome`).
pub async fn fetch_provider_models(
    request: ProviderModelFetchRequest,
) -> ProviderModelFetchOutcome {
    let ProviderModelFetchRequest {
        provider,
        api_base,
        api_key_env,
        api_key,
    } = request;
    // Short-circuit when the profile statically declares no catalog: the
    // facade resolves the API key **before** checking catalog support, so
    // without this check a keyless catalog-less provider (minimax / gitcode)
    // would report "configure XXX_API_KEY" — misleading, since even with the
    // key configured there is nothing to fetch.
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
    // Failures must leave a trace: silently degrading to the builtin list
    // would make users believe the official list is complete. The message
    // goes through the same sanitizing as the screen (no keys in logs).
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

/// Whether the provider profile explicitly declares "no official
/// model-list endpoint".
///
/// Unknown providers (absent from the registry) return `false`: the facade
/// then reports `unknown provider: …`, which is more informative than a
/// fabricated `Unsupported` here.
fn catalog_declared_unsupported(provider: &str) -> bool {
    xiaoo_api::llm::resolve_provider_profile(provider)
        .map(|profile| !profile.supports_model_catalog)
        .unwrap_or(false)
}

/// Display cap for externally sourced text: model ids and error messages
/// are one-liners; anything longer can only be an abnormal response or an
/// attack payload.
const MAX_EXTERNAL_TEXT_CHARS: usize = 200;

/// Maximum accepted model entries per provider. Official catalogs
/// normally run to tens or hundreds of entries (openrouter 300+); tens of
/// thousands can only be an abnormal endpoint or an attack payload — and
/// the list rebuilds its rows every frame, so rendering and memory need a
/// hard cap. Overflow is **silently truncated**: the header and status line
/// show the post-truncation count, with no extra hint.
const MAX_MODELS_PER_PROVIDER: usize = 2_000;

/// Invisible characters terminals "eat" or abuse for visual spoofing.
/// `char::is_control()` only covers Cc; this covers all of Unicode 15's Cf
/// (format) plus Zl / Zp (line / paragraph separators): a bidi override
/// (U+202E) can render a model name in a completely different character
/// order, and zero-width characters can make two distinct names look
/// identical.
fn is_invisible_or_directional(ch: char) -> bool {
    matches!(ch,
        '\u{00AD}'                      // soft hyphen
        | '\u{0600}'..='\u{0605}'       // Arabic number signs, etc.
        | '\u{061C}'                    // Arabic letter mark
        | '\u{06DD}' | '\u{070F}'       // Arabic / Syriac format marks
        | '\u{0890}'..='\u{0891}' | '\u{08E2}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'       // zero-width + bidi marks
        | '\u{2028}' | '\u{2029}'       // line / paragraph separators (Zl / Zp)
        | '\u{202A}'..='\u{202E}'       // bidi embedding / override
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'       // bidi isolates + deprecated format chars
        | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}' | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'     // Egyptian hieroglyph format controls
        | '\u{1BCA0}'..='\u{1BCA3}'     // shorthand format controls
        | '\u{1D173}'..='\u{1D17A}'     // musical notation format controls
        | '\u{E0001}' | '\u{E0020}'..='\u{E007F}' // tag characters
    )
}

/// Keep only the host part after `scheme://` (with port and IPv6 brackets):
/// path, query, fragment and userinfo are all stripped.
///
/// The query goes because provider keys may live there (gemini's `?key=`);
/// the host stays because "which endpoint failed" is the key fact when
/// debugging TLS / gateway issues — flattening everything to `<url>` would
/// make self-signed-gateway problems undebuggable. Userinfo (`user:pw@`) is
/// stripped for the same reason: passwords do not belong on screen.
///
/// Order: split the authority at `/` and strip userinfo first, then drop
/// path / query / fragment — so even a malformed URL (an unencoded `?`
/// inside userinfo) cannot leave a username or partial password on screen
/// masquerading as the host.
fn redact_url(token: &str) -> String {
    let Some((scheme, rest)) = token.split_once("://") else {
        return "<redacted>".to_string();
    };
    let slash_end = rest.find('/').unwrap_or(rest.len());
    let userinfo_end = rest[..slash_end].rfind('@').map_or(0, |at| at + 1);
    let after = &rest[userinfo_end..];
    let end = after.find(['/', '?', '#']).unwrap_or(after.len());
    format!("{scheme}://{}/<redacted>", &after[..end])
}

/// Fields shaped like `key=…` / `api_key: …` / `token=…` (case-insensitive).
fn looks_like_secret_field(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    [
        "key=",
        "key:",
        "token=",
        "token:",
        "secret=",
        "secret:",
        "password=",
        "password:",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Sanitizes **label-type** text from external endpoints (model ids,
/// display names, environment variable names) before it enters the TUI:
/// these strings get rendered into dialogs (`overlay.rs`), the status line
/// and session messages, and a malicious or broken endpoint could inject
/// control sequences through them (the classic TUI injection surface:
/// `\x1b` can clear the screen, retitle the terminal, forge keystrokes).
/// Keep only non-control characters outside Cf / Zl / Zp (see
/// `is_invisible_or_directional`), and cap the length.
fn sanitize_external_label(raw: &str) -> String {
    raw.chars()
        .filter(|ch| !ch.is_control() && !is_invisible_or_directional(*ch))
        .take(MAX_EXTERNAL_TEXT_CHARS)
        .collect()
}

/// Sanitizes **explanatory** text from external endpoints (error messages):
/// beyond control characters it also strips URLs — low-level client error
/// strings carry the request URL, and a provider URL's query may contain the
/// API key (rationale in [`redact_url`]), while this text is displayed on
/// screen.
///
/// Besides URLs it redacts whole `key=` / `key:` / `token=` / `token:` /
/// `secret=` / `secret:` / `password=` / `password:` tokens, and the single
/// token immediately following `Bearer` / `Basic`.
fn sanitize_external_message(raw: &str) -> String {
    // Replace control/invisible characters with spaces rather than deleting
    // them: deleting the newline in `...\nkey=xxx` would glue the pieces
    // into a single new token and defeat the token rules below.
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
            // Order matters: a full string with a scheme is treated as a
            // URL first (preserving "where it failed"), then as a bare
            // secret field.
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

/// `LlmModelSummary` → the TUI's `ModelInfo`: display name prefers the
/// official `display_name`, deduplicates by id, skips empty ids, and keeps
/// the official ordering (no sorting, no reshaping).
///
/// Both id and display name come from an external endpoint, so they pass
/// through [`sanitize_external_label`] before reaching the render layer.
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

/// When the official list lacks the currently active model, prepend it
/// (with a source annotation) — otherwise that model becomes unselectable
/// in the dialog and a casual Enter passively switches away from it.
///
/// Returns the list unchanged when `configured` is empty (unset / blank) or
/// already present (case-insensitive), so the common "the official list is
/// the whole truth" case is unaffected. The dialog copy and
/// `ChatState::available_providers` must use the same result to stay
/// consistent.
pub fn pin_configured_model(
    mut models: Vec<ModelInfo>,
    configured: Option<&str>,
) -> Vec<ModelInfo> {
    let Some(configured) = configured.map(str::trim).filter(|id| !id.is_empty()) else {
        return models;
    };
    if models
        .iter()
        .any(|model| model.id.eq_ignore_ascii_case(configured))
    {
        return models;
    }
    models.insert(
        0,
        ModelInfo {
            id: configured.to_string(),
            name: format!("{configured} (configured)"),
        },
    );
    models
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

/// Empty / all-whitespace → `None`, letting the facade fall back to the
/// provider profile's default base_url.
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
