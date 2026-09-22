use anyhow::Result;

use crate::app_state::AppState;
use crate::config::{save_llm_secret, Config};

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
        "xterm-kitty", "alacritty", "wezterm", "xterm-ghostty", "ghostty", "foot", "rio",
        "contour", "mintty",
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
        "vscode", "iTerm.app", "WezTerm", "ghostty", "mintty", "Hyper", "Tabby", "WarpTerminal",
    ];
    term_program.is_some_and(|program| KNOWN_PROGRAMS.iter().any(|known| program == *known))
}

pub fn copy_to_clipboard(text: &str) -> Result<ClipboardOutcome> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    // Wayland: wl-copy
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        if Command::new("wl-copy")
            .arg(text)
            .output()
            .ok()
            .map(|o| o.status.success())
            == Some(true)
        {
            return Ok(ClipboardOutcome::Native);
        }
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

#[cfg(test)]
#[path = "../../../../tests/unit/endside/services/provider_test.rs"]
mod tests;
