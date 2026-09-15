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

pub fn copy_to_clipboard(text: &str) -> Result<()> {
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
            return Ok(());
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
                return Ok(());
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
                return Ok(());
            }
        }
        // Fallback: arboard (only makes sense with a display server)
        if let Ok(mut clip) = arboard::Clipboard::new() {
            if clip.set_text(text).is_ok() {
                return Ok(());
            }
        }
    }

    // OSC 52 fallback: works in most modern terminals including Windows Terminal,
    // iTerm2, kitty, Alacritty, and over SSH.
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    // Write directly to stdout while still in raw mode.
    let mut out = std::io::stdout().lock();
    // When running inside tmux, wrap with a DCS passthrough so the outer
    // terminal receives the OSC 52 sequence (mirrors opencode's behaviour).
    if std::env::var("TMUX").is_ok() {
        write!(out, "\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\")?;
    } else {
        write!(out, "\x1b]52;c;{encoded}\x07")?;
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/services/provider_test.rs"]
mod tests;
