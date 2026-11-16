use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureFlags {
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
    /// L2 动态发现层开关：write pivot / 用户输入信号触发 skill 提示注入。
    /// 以 `skill_matching`（L1 清单层）开启为前提，两者同时开启才生效。
    #[serde(default = "default_true")]
    pub skill_discovery: bool,
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
            auto_compact: true,
            cache_editing: true,
            tool_execution: true,
            skill_matching: true,
            kvcache_enabled: false,
            kvcache_debug_enabled: false,
            redact_secrets_display: true,
            skill_discovery: true,
        }
    }
}
