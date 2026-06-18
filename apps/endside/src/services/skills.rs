use std::fmt::Write as _;

use agent_contracts::SkillRegistry;
use skill::FileSkillRegistry;

use crate::config::Config;

pub fn render_skills_overview(config: &Config) -> String {
    let skills_config = config.resolve_skills_config();
    let mut scanned_dirs: Vec<String> = skills_config
        .skills_dirs
        .iter()
        .map(|dir| dir.display().to_string())
        .collect();
    scanned_dirs.sort();
    scanned_dirs.dedup();

    let registry = FileSkillRegistry::new(&skills_config);
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
    use super::render_skills_overview;
    use crate::config::{Config, SkillsSection};
    use std::sync::{Mutex, OnceLock};
    use tempfile::TempDir;

    fn with_temp_home<T>(temp_home: &std::path::Path, test_fn: impl FnOnce() -> T) -> T {
        static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = HOME_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("home env lock should not be poisoned");

        let previous_home = std::env::var_os("HOME");
        std::env::set_var("HOME", temp_home);
        let result = test_fn();
        match previous_home {
            Some(previous_home) => std::env::set_var("HOME", previous_home),
            None => std::env::remove_var("HOME"),
        }
        result
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

        let mut config = Config::default();
        config.skills = Some(SkillsSection {
            dirs: Some(vec![skills_root.display().to_string()]),
            allow_scripts: Some(false),
        });

        with_temp_home(temp_dir.path(), || {
            let rendered = render_skills_overview(&config);
            assert!(rendered.contains("当前可用 skills（1）:"));
            assert!(rendered.contains("- reviewer: 审查当前改动"));
        });
    }

    #[test]
    fn render_skills_overview_shows_scanned_dirs_when_empty() {
        let temp_dir = TempDir::new().expect("create temp dir");
        let empty_root = temp_dir.path().join("empty-skills");
        std::fs::create_dir_all(&empty_root).expect("create empty skill root");

        let mut config = Config::default();
        config.skills = Some(SkillsSection {
            dirs: Some(vec![empty_root.display().to_string()]),
            allow_scripts: Some(false),
        });

        with_temp_home(temp_dir.path(), || {
            let rendered = render_skills_overview(&config);
            assert!(rendered.contains("当前未发现可用的 skills。"));
            assert!(rendered.contains(&empty_root.display().to_string()));
        });
    }
}
