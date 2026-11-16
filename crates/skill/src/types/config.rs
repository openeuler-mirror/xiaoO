use std::collections::HashSet;
use std::path::PathBuf;

/// L1 清单 description 截断上限的缺省值（Unicode 标量数）。
/// [`SkillsConfig::max_listing_description_chars`] 的 `Default` 实现与各端
/// `SkillsSection` 解析的回落值共用此常量，防默认值漂移
pub const DEFAULT_MAX_LISTING_DESCRIPTION_CHARS: usize = 250;

#[derive(Debug, Clone)]
pub struct SkillsConfig {
    pub skills_dirs: Vec<PathBuf>,
    pub allow_scripts: bool,
    pub disabled: HashSet<String>,
    /// Enable security audit on skill directories before loading. Default: false.
    pub audit_enabled: bool,
    pub prompt_injection_mode: PromptInjectionMode,
    pub prompt_budget_ratio: f64,
    pub max_listing_description_chars: usize,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            skills_dirs: Vec::new(),
            allow_scripts: false,
            disabled: HashSet::new(),
            audit_enabled: false,
            prompt_injection_mode: PromptInjectionMode::Compact,
            prompt_budget_ratio: 0.01,
            max_listing_description_chars: DEFAULT_MAX_LISTING_DESCRIPTION_CHARS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptInjectionMode {
    Full,
    Compact,
}
