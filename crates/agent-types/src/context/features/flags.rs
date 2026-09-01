use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureFlags {
    pub history_snip: bool,
    pub context_collapse: bool,
    pub auto_compact: bool,
    pub cache_editing: bool,
    pub tool_execution: bool,
    pub skill_matching: bool,
    #[serde(default = "default_false")]
    pub kvcache_enabled: bool,
    #[serde(default = "default_false")]
    pub kvcache_debug_enabled: bool,
    /// Whether assistant display text is redacted for echoed secrets. Only
    /// guards the display sink path (not history/snapshot, which already hold
    /// raw secrets). Default `true` so the daemon/SSE path always redacts; the
    /// local TUI overrides this from its config (default `false`).
    #[serde(default = "default_true")]
    pub redact_secrets_display: bool,
}

fn default_false() -> bool {
    false
}

fn default_true() -> bool {
    true
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            history_snip: true,
            context_collapse: true,
            auto_compact: true,
            cache_editing: true,
            tool_execution: true,
            skill_matching: true,
            kvcache_enabled: false,
            kvcache_debug_enabled: false,
            redact_secrets_display: true,
        }
    }
}
