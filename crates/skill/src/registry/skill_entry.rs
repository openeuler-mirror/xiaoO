use std::path::Path;

use agent_contracts::skill::registry::SkillContext;
use agent_contracts::SkillSpec;

use crate::types::Skill;

/// Adapter: wraps a [Skill] domain object to implement [SkillSpec] trait.
pub struct SkillEntry {
    pub(crate) skill: Skill,
}

impl SkillEntry {
    pub fn new(skill: Skill) -> Self {
        Self { skill }
    }
}

impl SkillSpec for SkillEntry {
    fn skill_id(&self) -> &str {
        &self.skill.name
    }

    fn description(&self) -> &str {
        &self.skill.description
    }

    fn full_prompt(&self) -> &str {
        &self.skill.prompt
    }

    fn paths(&self) -> &[String] {
        &self.skill.paths
    }

    fn user_invocable(&self) -> bool {
        self.skill.user_invocable
    }

    fn disable_model_invocation(&self) -> bool {
        self.skill.disable_model_invocation
    }

    fn arguments(&self) -> &[String] {
        &self.skill.arguments
    }

    fn argument_hint(&self) -> Option<&str> {
        self.skill.argument_hint.as_deref()
    }

    fn context(&self) -> SkillContext {
        self.skill.context
    }

    fn location(&self) -> Option<&Path> {
        self.skill.location.as_deref()
    }
}
