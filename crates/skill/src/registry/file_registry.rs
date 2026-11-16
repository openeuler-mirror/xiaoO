use std::sync::Arc;

use agent_contracts::{SkillRegistry, SkillSpec};
use agent_types::common::truncate_scalars;
use agent_types::context::prompt::SkillSummary;

use crate::loading::load_skills;
use crate::types::config::SkillsConfig;
use crate::types::Skill;

use super::skill_entry::SkillEntry;

/// File-based skill registry that loads skills from disk.
pub struct FileSkillRegistry {
    entries: Vec<SkillEntry>,
    /// L1 清单段快照：构造时一次性完成新字段填充与 description 截断，
    /// `list_skills()` 只做 `Arc` 克隆。registry 会话期不可变，任意轮次
    /// 产出相同清单字节（system prompt 前缀稳定）由结构保证
    summaries: Arc<[SkillSummary]>,
}

impl FileSkillRegistry {
    pub fn new(config: &SkillsConfig) -> Self {
        Self::build(load_skills(config), config.max_listing_description_chars)
    }

    /// 与 [`FileSkillRegistry::new`] 同规则构建，但 skill 清单来自调用方
    /// （e2b 远端解析等场景）。截断上限由调用方显式给出，避免隐式默认值
    /// 覆盖用户配置的 `max_listing_description_chars`
    pub fn from_skills_with_limit(
        skills: Vec<Skill>,
        max_listing_description_chars: usize,
    ) -> Self {
        Self::build(skills, max_listing_description_chars)
    }

    fn build(skills: Vec<Skill>, max_listing_description_chars: usize) -> Self {
        let entries: Vec<SkillEntry> = skills.into_iter().map(SkillEntry::new).collect();
        let summaries: Arc<[SkillSummary]> = entries
            .iter()
            .map(|entry| SkillSummary {
                skill_id: entry.skill.name.clone(),
                description: truncate_scalars(
                    &entry.skill.description,
                    max_listing_description_chars,
                ),
                when_to_use: entry.skill.when_to_use.clone(),
                paths: entry.skill.paths.clone(),
                tags: entry.skill.tags.clone(),
                disable_model_invocation: entry.skill.disable_model_invocation,
            })
            .collect();
        Self { entries, summaries }
    }
}

impl SkillRegistry for FileSkillRegistry {
    fn list_skills(&self) -> Arc<[SkillSummary]> {
        Arc::clone(&self.summaries)
    }

    fn get_skill(&self, skill_id: &str) -> Option<&dyn SkillSpec> {
        self.entries
            .iter()
            .find(|e| e.skill.name == skill_id)
            .map(|e| e as &dyn SkillSpec)
    }
}
