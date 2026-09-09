use crate::daemon_config::DaemonConfig;
use serde::Serialize;
use std::path::{Path, PathBuf};
use xiaoo_api::skills::{FileSkillRegistry, SkillRegistry};

#[derive(Debug, Serialize)]
pub struct SkillCatalogReport {
    pub schema_version: u32,
    pub allow_scripts: bool,
    pub directories: Vec<SkillDirectorySummary>,
    pub skills: Vec<SkillSummary>,
}

#[derive(Debug, Serialize)]
pub struct SkillDirectorySummary {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Serialize)]
pub struct SkillSummary {
    pub id: String,
    pub enabled: bool,
    pub description: String,
    pub location: Option<String>,
    pub context: String,
    pub user_invocable: bool,
    pub disable_model_invocation: bool,
    pub paths: Vec<String>,
    pub arguments: Vec<String>,
    pub argument_hint: Option<String>,
}

pub fn skill_catalog(config: &DaemonConfig, workspace: &Path) -> SkillCatalogReport {
    let mut skills_config = config.resolve_skills_config();
    skills_config.skills_dirs = skills_config
        .skills_dirs
        .into_iter()
        .map(|path| resolve_directory(path, workspace))
        .collect();
    let disabled = skills_config.disabled.clone();
    let mut discovery_config = skills_config.clone();
    discovery_config.disabled.clear();
    let registry = FileSkillRegistry::new(&discovery_config);
    let skills = registry
        .list_skills()
        .into_iter()
        .filter_map(|summary| {
            let skill = registry.get_skill(&summary.skill_id)?;
            Some(SkillSummary {
                id: summary.skill_id,
                enabled: !disabled.contains(skill.skill_id()),
                description: summary.description,
                location: skill.location().map(|path| path.display().to_string()),
                context: format!("{:?}", skill.context()).to_lowercase(),
                user_invocable: skill.user_invocable(),
                disable_model_invocation: skill.disable_model_invocation(),
                paths: skill.paths().to_vec(),
                arguments: skill.arguments().to_vec(),
                argument_hint: skill.argument_hint().map(str::to_string),
            })
        })
        .collect();
    SkillCatalogReport {
        schema_version: 1,
        allow_scripts: skills_config.allow_scripts,
        directories: skills_config
            .skills_dirs
            .iter()
            .map(|path| SkillDirectorySummary {
                path: path.display().to_string(),
                exists: path.is_dir(),
            })
            .collect(),
        skills,
    }
}

fn resolve_directory(path: PathBuf, workspace: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::skill_catalog;
    use crate::daemon_config::DaemonConfig;
    use tempfile::TempDir;

    #[test]
    fn reports_effective_skill_metadata_and_directories() {
        let temp = TempDir::new().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let skill_dir = temp.path().join(".xiaoo/skills/reviewer");
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: reviewer\ndescription: Review code\nuser_invocable: false\n---\nReview carefully.\n",
        )
        .expect("skill");
        std::fs::write(
            &config_path,
            "[llm]\nactive_profile = \"local\"\n\n[llm.profiles.local]\nprovider = \"ollama\"\nmodel = \"qwen3\"\n\n[skills]\ndisabled = [\"reviewer\"]\n",
        )
        .expect("config");
        let config = DaemonConfig::load_from(&config_path).expect("load config");
        let report = skill_catalog(&config, temp.path());

        assert_eq!(report.schema_version, 1);
        assert!(report.directories[0].exists);
        let reviewer = report
            .skills
            .iter()
            .find(|skill| skill.id == "reviewer")
            .expect("reviewer");
        assert_eq!(reviewer.description, "Review code");
        assert!(!reviewer.enabled);
        assert!(!reviewer.user_invocable);
        assert_eq!(reviewer.context, "inline");
    }
}
