use std::fmt::Write as _;

use skill::SkillsConfig;
use xiaoo_api::skills::{FileSkillRegistry, SkillRegistry};

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
mod tests {
    use super::render_skills_overview_with_config;
    use skill::SkillsConfig;
    use tempfile::TempDir;

    fn skills_config_with_dirs(dirs: Vec<std::path::PathBuf>) -> SkillsConfig {
        SkillsConfig {
            skills_dirs: dirs,
            ..SkillsConfig::default()
        }
    }

    #[test]
    fn render_skills_overview_lists_loaded_skills() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let skills_root = temp_dir.path().join("skills");
        let review_dir = skills_root.join("reviewer");
        std::fs::create_dir_all(&review_dir).expect("create skill dir");
        std::fs::write(
            review_dir.join("SKILL.md"),
            "---\ndescription: 审查当前改动\n---\nReview the current patch.",
        )
        .expect("write skill file");

        let config = skills_config_with_dirs(vec![skills_root]);
        let rendered = render_skills_overview_with_config(&config);
        assert!(rendered.contains("当前可用 skills（1）:"));
        assert!(rendered.contains("- reviewer: 审查当前改动"));
    }

    #[test]
    fn render_skills_overview_shows_scanned_dirs_when_empty() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let empty_root = temp_dir.path().join("empty-skills");
        std::fs::create_dir_all(&empty_root).expect("create empty skill root");

        let config = skills_config_with_dirs(vec![empty_root.clone()]);
        let rendered = render_skills_overview_with_config(&config);
        assert!(rendered.contains("当前未发现可用的 skills。"));
        assert!(rendered.contains(&empty_root.display().to_string()));
    }
}
