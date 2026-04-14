use std::collections::HashMap;
use std::path::Path;

use agent_contracts::SkillContext;
use serde::Deserialize;

use crate::error::SkillError;
use crate::types::{Skill, SkillToolDef, SkillToolKind};

/// Parse a SKILL.toml file into a Skill.
pub fn load_skill_toml(path: &Path, skill_dir: &Path) -> Result<Skill, SkillError> {
    let content = std::fs::read_to_string(path).map_err(|e| SkillError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    let manifest: SkillManifest = toml::from_str(&content).map_err(|e| SkillError::TomlParse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;

    let skill_section = manifest.skill;

    let name = skill_section.name.ok_or_else(|| SkillError::MissingField {
        path: path.to_path_buf(),
        field: "skill.name".into(),
    })?;

    let context = match skill_section.context.as_deref() {
        Some("fork") => SkillContext::Fork,
        _ => SkillContext::Inline,
    };

    let tools: Vec<SkillToolDef> = manifest
        .tools
        .unwrap_or_default()
        .into_iter()
        .map(|t| SkillToolDef {
            name: t.name,
            description: t.description.unwrap_or_default(),
            kind: match t.kind.as_str() {
                "http" => SkillToolKind::Http,
                "script" => SkillToolKind::Script,
                _ => SkillToolKind::Shell,
            },
            command: t.command,
            args: t.args.unwrap_or_default(),
        })
        .collect();

    // Read prompt from companion SKILL.md body if it exists
    let prompt_md_path = skill_dir.join("SKILL.md");
    let prompt = if prompt_md_path.exists() {
        let content = std::fs::read_to_string(&prompt_md_path).unwrap_or_default();
        // Extract body after frontmatter if present
        let trimmed = content.trim_start();
        if trimmed.starts_with("---") {
            let after_first = &trimmed[3..];
            if let Some(end_pos) = after_first.find("\n---") {
                let rest_start = end_pos + 4;
                if rest_start < after_first.len() {
                    let rest = &after_first[rest_start..];
                    rest.strip_prefix('\n').unwrap_or(rest).to_string()
                } else {
                    String::new()
                }
            } else {
                content
            }
        } else {
            content
        }
    } else {
        skill_section
            .prompts
            .map(|p| p.join("\n\n"))
            .unwrap_or_default()
    };

    Ok(Skill {
        name,
        description: skill_section.description.unwrap_or_default(),
        version: skill_section.version,
        author: skill_section.author,
        tags: skill_section.tags.unwrap_or_default(),
        location: Some(skill_dir.to_path_buf()),
        prompt,
        user_invocable: skill_section.user_invocable.unwrap_or(true),
        disable_model_invocation: skill_section.disable_model_invocation.unwrap_or(false),
        context,
        argument_hint: skill_section.argument_hint,
        arguments: skill_section.arguments.unwrap_or_default(),
        paths: skill_section.paths.unwrap_or_default(),
        tools,
    })
}

#[derive(Deserialize)]
struct SkillManifest {
    skill: SkillSection,
    #[serde(rename = "tools")]
    tools: Option<Vec<ToolEntry>>,
}

#[derive(Deserialize)]
struct SkillSection {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<String>,
    tags: Option<Vec<String>>,
    user_invocable: Option<bool>,
    disable_model_invocation: Option<bool>,
    context: Option<String>,
    argument_hint: Option<String>,
    arguments: Option<Vec<String>>,
    paths: Option<Vec<String>>,
    prompts: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ToolEntry {
    name: String,
    description: Option<String>,
    kind: String,
    command: String,
    args: Option<HashMap<String, String>>,
}
