use std::collections::HashSet;
use std::path::Path;

use crate::audit::{audit_skill_directory, SkillAuditOptions};
use crate::types::config::SkillsConfig;
use crate::types::Skill;

use super::md_parser::load_skill_md;
use super::toml_parser::load_skill_toml;

/// Load skills from all configured directories.
///
/// Scans each directory for subdirectories containing SKILL.toml or SKILL.md.
/// Deduplicates by name — first loaded wins.
pub fn load_skills(config: &SkillsConfig) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut seen_names = HashSet::new();

    for dir in &config.skills_dirs {
        if !dir.is_dir() {
            tracing::debug!(dir = %dir.display(), "skills directory does not exist, skipping");
            continue;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), error = %e, "failed to read skills directory");
                continue;
            }
        };

        let audit_options = SkillAuditOptions {
            allow_scripts: config.allow_scripts,
            ..SkillAuditOptions::default()
        };

        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if !skill_dir.is_dir() {
                continue;
            }

            // Security audit before loading (when enabled)
            if config.audit_enabled {
                let audit_report = audit_skill_directory(&skill_dir, &audit_options);
                if !audit_report.is_clean() {
                    tracing::warn!(
                        dir = %skill_dir.display(),
                        findings = ?audit_report.findings,
                        "skill failed security audit, skipping"
                    );
                    continue;
                }
            }

            match load_skill_from_dir(&skill_dir) {
                Ok(skill) => {
                    if seen_names.contains(&skill.name) {
                        tracing::debug!(
                            name = %skill.name,
                            dir = %skill_dir.display(),
                            "duplicate skill name, skipping"
                        );
                        continue;
                    }
                    tracing::debug!(
                        name = %skill.name,
                        dir = %skill_dir.display(),
                        "loaded skill"
                    );
                    seen_names.insert(skill.name.clone());
                    skills.push(skill);
                }
                Err(e) => {
                    tracing::warn!(
                        dir = %skill_dir.display(),
                        error = %e,
                        "failed to load skill, skipping"
                    );
                }
            }
        }
    }

    skills
}

/// Load a single skill from a directory.
///
/// Prefers SKILL.toml over SKILL.md.
fn load_skill_from_dir(skill_dir: &Path) -> Result<Skill, crate::error::SkillError> {
    let toml_path = skill_dir.join("SKILL.toml");
    let md_path = skill_dir.join("SKILL.md");

    let mut skill = if toml_path.exists() {
        load_skill_toml(&toml_path, skill_dir)?
    } else if md_path.exists() {
        load_skill_md(&md_path, skill_dir)?
    } else {
        return Err(crate::error::SkillError::MissingField {
            path: skill_dir.to_path_buf(),
            field: "SKILL.md or SKILL.toml".into(),
        });
    };

    skill.prompt = format!(
        "{}\n<indicator>skill loaded from {}</indicator>",
        skill.prompt,
        skill_dir.display()
    );

    Ok(skill)
}
