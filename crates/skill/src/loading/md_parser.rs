use std::path::Path;

use agent_contracts::SkillContext;

use crate::error::SkillError;
use crate::types::Skill;

/// Parse a SKILL.md file into a Skill.
///
/// Format: optional YAML frontmatter delimited by `---`, followed by the prompt body.
pub fn load_skill_md(path: &Path, skill_dir: &Path) -> Result<Skill, SkillError> {
    let content = std::fs::read_to_string(path).map_err(|e| SkillError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (frontmatter, body) = split_frontmatter(&content);
    let meta = parse_frontmatter(frontmatter, path)?;

    let name = meta
        .name
        .or_else(|| {
            skill_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .ok_or_else(|| SkillError::MissingField {
            path: path.to_path_buf(),
            field: "name".into(),
        })?;

    // If no description in frontmatter, extract from body:
    // use first non-empty, non-heading line as description.
    let description = meta
        .description
        .unwrap_or_else(|| extract_description_from_body(body));

    Ok(Skill {
        name,
        description,
        version: meta.version,
        author: meta.author,
        tags: meta.tags,
        location: Some(skill_dir.to_path_buf()),
        prompt: body.to_string(),
        user_invocable: meta.user_invocable.unwrap_or(true),
        disable_model_invocation: meta.disable_model_invocation.unwrap_or(false),
        context: meta.context.unwrap_or(SkillContext::Inline),
        argument_hint: meta.argument_hint,
        arguments: meta.arguments,
        paths: meta.paths,
        tools: Vec::new(),
    })
}

fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }

    // Skip leading "---\n"
    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    if let Some(end_pos) = after_first.find("\n---") {
        let fm = &after_first[..end_pos];
        let rest_start = end_pos + 4; // skip "\n---"
        let body = if rest_start < after_first.len() {
            let rest = &after_first[rest_start..];
            rest.strip_prefix('\n').unwrap_or(rest)
        } else {
            ""
        };
        (Some(fm), body)
    } else {
        (None, content)
    }
}

#[derive(Default)]
struct FrontmatterMeta {
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    author: Option<String>,
    tags: Vec<String>,
    user_invocable: Option<bool>,
    disable_model_invocation: Option<bool>,
    context: Option<SkillContext>,
    argument_hint: Option<String>,
    arguments: Vec<String>,
    paths: Vec<String>,
}

fn parse_frontmatter(fm: Option<&str>, path: &Path) -> Result<FrontmatterMeta, SkillError> {
    let fm = match fm {
        Some(s) if !s.trim().is_empty() => s,
        _ => return Ok(FrontmatterMeta::default()),
    };

    let value: serde_json::Value =
        serde_json::from_str(&yaml_like_to_json(fm)).map_err(|e| SkillError::FrontmatterParse {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;

    let obj = value
        .as_object()
        .ok_or_else(|| SkillError::FrontmatterParse {
            path: path.to_path_buf(),
            message: "frontmatter must be a YAML mapping".into(),
        })?;

    let mut meta = FrontmatterMeta::default();

    meta.name = get_str(obj, "name");
    meta.description = get_str(obj, "description");
    meta.version = get_str(obj, "version");
    meta.author = get_str(obj, "author");
    meta.argument_hint = get_str(obj, "argument_hint").or_else(|| get_str(obj, "argument-hint"));
    meta.tags = get_string_list(obj, "tags");
    meta.arguments = get_string_list(obj, "arguments");
    meta.paths = get_string_list(obj, "paths");

    meta.user_invocable =
        get_bool(obj, "user_invocable").or_else(|| get_bool(obj, "user-invocable"));
    meta.disable_model_invocation = get_bool(obj, "disable_model_invocation")
        .or_else(|| get_bool(obj, "disable-model-invocation"));

    if let Some(ctx) = get_str(obj, "context") {
        meta.context = match ctx.as_str() {
            "inline" => Some(SkillContext::Inline),
            "fork" => Some(SkillContext::Fork),
            _ => None,
        };
    }

    Ok(meta)
}

/// Extract a description from the markdown body when frontmatter has none.
///
/// Strategy: take the first non-empty line that isn't a heading (`#`),
/// a horizontal rule (`---`), or a code fence. Truncate to 200 chars.
fn extract_description_from_body(body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("---")
            || trimmed.starts_with("```")
        {
            continue;
        }
        let desc: String = trimmed.chars().take(200).collect();
        return desc;
    }
    String::new()
}

/// Minimal YAML-like to JSON converter for simple key: value frontmatter.
/// Handles: strings, booleans, simple inline arrays `[a, b, c]`, and multiline strings (`>` or `|`).
fn yaml_like_to_json(yaml: &str) -> String {
    let mut entries = Vec::new();
    let lines: Vec<&str> = yaml.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().trim_matches('"');
            let value = value.trim();

            let json_value = if value == ">" || value == "|" {
                let fold = value == ">";
                let mut multiline_content = String::new();
                let base_indent = if i + 1 < lines.len() {
                    lines[i + 1].chars().take_while(|c| *c == ' ').count()
                } else {
                    0
                };

                let mut j = i + 1;
                while j < lines.len() {
                    let next_line = lines[j];
                    let trimmed_next = next_line.trim();

                    if trimmed_next.is_empty() {
                        if fold {
                            if !multiline_content.is_empty() && !multiline_content.ends_with(' ') {
                                multiline_content.push(' ');
                            }
                        } else {
                            multiline_content.push('\n');
                        }
                        j += 1;
                        continue;
                    }

                    let current_indent = next_line.chars().take_while(|c| *c == ' ').count();

                    if current_indent < base_indent && !next_line.starts_with(' ') {
                        break;
                    }

                    let content = if current_indent >= base_indent {
                        &next_line[base_indent.min(next_line.len())..]
                    } else {
                        trimmed_next
                    };

                    if fold {
                        if !multiline_content.is_empty() && !multiline_content.ends_with(' ') {
                            multiline_content.push(' ');
                        }
                        multiline_content.push_str(content);
                    } else {
                        if !multiline_content.is_empty() {
                            multiline_content.push('\n');
                        }
                        multiline_content.push_str(content);
                    }
                    j += 1;
                }
                i = j - 1;

                let content = multiline_content.trim();
                format!("\"{}\"", content.replace('\\', "\\\\").replace('"', "\\\""))
            } else if value.starts_with('[') && value.ends_with(']') {
                let inner = &value[1..value.len() - 1];
                let items: Vec<String> = inner
                    .split(',')
                    .map(|s| {
                        let s = s.trim().trim_matches('"').trim_matches('\'');
                        format!("\"{}\"", s)
                    })
                    .collect();
                format!("[{}]", items.join(","))
            } else if value == "true" || value == "false" {
                value.to_string()
            } else if value.is_empty() {
                "null".to_string()
            } else {
                let unquoted = value.trim_matches('"').trim_matches('\'');
                format!(
                    "\"{}\"",
                    unquoted.replace('\\', "\\\\").replace('"', "\\\"")
                )
            };

            entries.push(format!("\"{}\":{}", key, json_value));
        }
        i += 1;
    }

    format!("{{{}}}", entries.join(","))
}

fn get_str(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn get_bool(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<bool> {
    obj.get(key).and_then(|v| v.as_bool())
}

fn get_string_list(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Vec<String> {
    obj.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_with_yaml() {
        let content = "---\nname: test\ndescription: hello\n---\nBody here";
        let (fm, body) = split_frontmatter(content);
        assert_eq!(fm, Some("name: test\ndescription: hello"));
        assert_eq!(body, "Body here");
    }

    #[test]
    fn split_frontmatter_without_yaml() {
        let content = "Just a body without frontmatter";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn extract_description_from_body_skips_headings() {
        let body = "\n# My Skill\n\nThis does something useful.\n\nMore details.";
        assert_eq!(
            extract_description_from_body(body),
            "This does something useful."
        );
    }

    #[test]
    fn extract_description_empty_body() {
        assert_eq!(extract_description_from_body(""), "");
        assert_eq!(extract_description_from_body("# Only heading"), "");
    }

    #[test]
    fn yaml_like_to_json_basic() {
        let yaml = "name: test\nversion: \"1.0\"\nuser-invocable: true\ntags: [a, b, c]";
        let json = yaml_like_to_json(yaml);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["name"], "test");
        assert_eq!(v["version"], "1.0");
        assert_eq!(v["user-invocable"], true);
        assert_eq!(v["tags"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn yaml_like_to_json_multiline_folded() {
        let yaml = "name: test\ndescription: >\n  Line one.\n  Line two.\n  Line three.";
        let json = yaml_like_to_json(yaml);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["name"], "test");
        assert_eq!(v["description"], "Line one. Line two. Line three.");
    }

    #[test]
    fn yaml_like_to_json_multiline_literal() {
        let yaml = "name: test\ndescription: |\n  Line one.\n  Line two.";
        let json = yaml_like_to_json(yaml);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["name"], "test");
        assert_eq!(v["description"], "Line one.\nLine two.");
    }
}
