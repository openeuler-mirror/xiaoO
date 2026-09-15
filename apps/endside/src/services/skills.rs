use std::fmt::Write as _;

use xiaoo_api::skills::{FileSkillRegistry, SkillRegistry};
use xiaoo_shared::skills_support::SkillsConfig;

use crate::config::Config;

pub fn render_skills_overview(config: &Config) -> String {
    render_skills_overview_with_config(&config.resolve_skills_config())
}

pub fn render_skills_overview_with_config(skills_config: &SkillsConfig) -> String {
    let mut scanned_dirs: Vec<String> = skills_config
        .skills_dirs
        .iter()
        .map(|dir| dir.display().to_string())
        .collect();
    scanned_dirs.sort();
    scanned_dirs.dedup();

    let registry = FileSkillRegistry::new(skills_config);
    let mut skills = registry.list_skills();
    skills.sort_by(|left, right| left.skill_id.cmp(&right.skill_id));

    if skills.is_empty() {
        let mut output = String::from("当前未发现可用的 skills。");
        if !scanned_dirs.is_empty() {
            output.push_str("\n扫描目录:");
            for dir in scanned_dirs {
                let _ = write!(output, "\n- {dir}");
            }
        }
        return output;
    }

    let mut output = format!("当前可用 skills（{}）:", skills.len());
    for skill in skills {
        let description = if skill.description.trim().is_empty() {
            "无描述"
        } else {
            skill.description.trim()
        };
        let _ = write!(output, "\n- {}: {}", skill.skill_id, description);
    }

    output
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/services/skills_test.rs"]
mod tests;
