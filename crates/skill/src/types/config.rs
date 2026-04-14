use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SkillsConfig {
    pub skills_dirs: Vec<PathBuf>,
    pub allow_scripts: bool,
    pub prompt_injection_mode: PromptInjectionMode,
    pub prompt_budget_ratio: f64,
    pub max_listing_description_chars: usize,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            skills_dirs: Vec::new(),
            allow_scripts: false,
            prompt_injection_mode: PromptInjectionMode::Compact,
            prompt_budget_ratio: 0.01,
            max_listing_description_chars: 250,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptInjectionMode {
    Full,
    Compact,
}
