use agent_contracts::{SkillRegistry, SkillSpec};
use agent_types::context::prompt::SkillSummary;

use crate::loading::load_skills;
use crate::types::config::SkillsConfig;

use super::skill_entry::SkillEntry;

/// File-based skill registry that loads skills from disk.
pub struct FileSkillRegistry {
    entries: Vec<SkillEntry>,
}

impl FileSkillRegistry {
    pub fn new(config: &SkillsConfig) -> Self {
        let skills = load_skills(config);
        let entries = skills.into_iter().map(SkillEntry::new).collect();
        Self { entries }
    }
}

impl SkillRegistry for FileSkillRegistry {
    fn list_skills(&self) -> Vec<SkillSummary> {
        self.entries
            .iter()
            .map(|entry| SkillSummary {
                skill_id: entry.skill.name.clone(),
                description: entry.skill.description.clone(),
            })
            .collect()
    }

    fn get_skill(&self, skill_id: &str) -> Option<&dyn SkillSpec> {
        self.entries
            .iter()
            .find(|e| e.skill.name == skill_id)
            .map(|e| e as &dyn SkillSpec)
    }
}
