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
    /// 场景提示（何时该用这个 skill），可缺省
    pub when_to_use: Option<String>,
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
        .iter()
        .filter_map(|summary| {
            let skill = registry.get_skill(&summary.skill_id)?;
            Some(SkillSummary {
                id: summary.skill_id.clone(),
                enabled: !disabled.contains(skill.skill_id()),
                // spec 访问器取全文：L1 清单截断（max_listing_description_chars）
                // 只作用于 prompt 视图，管理侧目录不截断
                description: skill.description().to_string(),
                when_to_use: skill.when_to_use().map(str::to_string),
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
#[path = "../../../tests/unit/serverside/skill_management_test.rs"]
mod tests;
