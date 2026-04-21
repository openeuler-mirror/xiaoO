use crate::render::utils::sanitize_terminal_text;

use std::path::Path;

pub struct StatusPanel {
    pub model_name: String,
    pub provider_name: String,
    /// Shortened display string for current agent workspace (tools cwd).
    pub workspace_display: String,
    pub total_tokens: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub last_latency_ms: u64,
    pub is_connected: bool,
    pub input_context_tokens: u64,
    pub context_window_tokens: u64,
}

impl Default for StatusPanel {
    fn default() -> Self {
        Self {
            model_name: String::new(),
            provider_name: String::new(),
            workspace_display: String::new(),
            total_tokens: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            last_latency_ms: 0,
            is_connected: false,
            input_context_tokens: 0,
            context_window_tokens: 0,
        }
    }
}

impl StatusPanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_provider(&mut self, provider: &str, model: &str) {
        self.provider_name = provider.to_string();
        self.model_name = model.to_string();
        self.is_connected = true;
    }

    pub fn set_context_window(&mut self, context_window_tokens: u64) {
        self.context_window_tokens = context_window_tokens;
    }

    /// Update the workspace line from an absolute path (caller should pass canonical path when possible).
    pub fn set_workspace(&mut self, path: &Path) {
        self.workspace_display = shorten_path_display(path, 26);
    }

    pub fn update_metrics(
        &mut self,
        prompt_tokens: u64,
        completion_tokens: u64,
        latency_ms: u64,
        input_context_tokens: u64,
    ) {
        self.input_context_tokens = input_context_tokens;
        self.prompt_tokens = self.prompt_tokens.saturating_add(prompt_tokens);
        self.completion_tokens = self.completion_tokens.saturating_add(completion_tokens);
        self.total_tokens = self
            .total_tokens
            .saturating_add(prompt_tokens.saturating_add(completion_tokens));

        self.last_latency_ms = latency_ms;
    }

    pub(crate) fn format_token_count(tokens: u64) -> String {
        if tokens < 1000 {
            format!("{}", tokens)
        } else if tokens < 1_000_000 {
            format!("{:.1}K", tokens as f64 / 1000.0)
        } else {
            format!("{:.1}M", tokens as f64 / 1_000_000.0)
        }
    }

    pub(crate) fn format_context_usage(input_tokens: u64, context_window_tokens: u64) -> String {
        if context_window_tokens == 0 {
            return Self::format_token_count(input_tokens);
        }

        format!(
            "{}/{}",
            Self::format_token_count(input_tokens),
            Self::format_token_count(context_window_tokens)
        )
    }
}

fn shorten_path_display(path: &Path, max_chars: usize) -> String {
    let s = path.to_string_lossy();
    let count = s.chars().count();
    if count <= max_chars {
        return s.into_owned();
    }
    let prefix = sanitize_terminal_text("…");
    let keep = max_chars.saturating_sub(prefix.chars().count());
    let skip = count.saturating_sub(keep);
    format!("{prefix}{}", s.chars().skip(skip).collect::<String>())
}
