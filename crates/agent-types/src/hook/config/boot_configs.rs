use crate::common::HookerId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default cross-turn `send_prompt` chain depth cap (exclusive upper bound
/// on `chain_depth`). A plugin's `*.Session.lifecycle.state` hooker may
/// request a `SendPrompt` action; the resulting turn fires the state hook
/// again, which may emit another `SendPrompt`, forming a chain. The daemon
/// stamps each `SendPrompt` with `chain_depth = emitting_turn_depth + 1`
/// and drops the action once the stamped value **reaches** this cap
/// (`next_depth >= max_prompt_chain_depth`). Semantics: `N` permits a
/// chain of **N turns total** — the user-initiated turn at depth `0` plus
/// `N - 1` `send_prompt`-triggered turns (depths `1..=N-1`); the turn
/// that would run at depth `N` is dropped. A normal user-typed turn
/// resets the chain (`chain_depth = 0`). Configurable via
/// `[hooker].max_prompt_chain_depth`.
pub const DEFAULT_MAX_PROMPT_CHAIN_DEPTH: usize = 128;

fn default_max_prompt_chain_depth() -> usize {
    DEFAULT_MAX_PROMPT_CHAIN_DEPTH
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum HookerDefaultMode {
    #[default]
    #[serde(alias = "all", alias = "ALL")]
    All,
    #[serde(alias = "none", alias = "NONE")]
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookerRegistryConfig {
    #[serde(default)]
    pub default: HookerDefaultMode,
    #[serde(default)]
    pub enabled: Vec<HookerId>,
    #[serde(default)]
    pub disabled: Vec<HookerId>,
    #[serde(default)]
    pub policies: HashMap<HookerId, serde_json::Value>,
    #[serde(default)]
    pub plugins: Vec<String>,
    /// Cross-turn `send_prompt` chain depth cap (exclusive upper bound on
    /// `chain_depth`). The daemon drops a `SendPrompt` action when the
    /// next-turn depth would **reach** this value
    /// (`next_depth >= max_prompt_chain_depth`); `N` permits N turns total
    /// in a chain (depths `0..=N-1`). See
    /// [`DEFAULT_MAX_PROMPT_CHAIN_DEPTH`].
    #[serde(default = "default_max_prompt_chain_depth")]
    pub max_prompt_chain_depth: usize,
}

impl Default for HookerRegistryConfig {
    fn default() -> Self {
        Self {
            default: HookerDefaultMode::default(),
            enabled: Vec::new(),
            disabled: Vec::new(),
            policies: HashMap::new(),
            plugins: Vec::new(),
            max_prompt_chain_depth: DEFAULT_MAX_PROMPT_CHAIN_DEPTH,
        }
    }
}
