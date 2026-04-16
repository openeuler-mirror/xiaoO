use std::path::PathBuf;

/// An external command loaded from `~/.xiaoo/command/<name>.md`.
#[derive(Debug, Clone)]
pub struct ExternalCommand {
    /// Command name derived from filename (without `.md`).
    pub name: String,
    /// Short description from frontmatter `description` field.
    pub description: String,
    /// Markdown body after the frontmatter — injected as user input when selected.
    pub body: String,
}

/// Scan `~/.xiaoo/command/` and load all `.md` command files.
///
/// Returns an empty Vec if the directory does not exist or cannot be read.
pub fn load_external_commands() -> Vec<ExternalCommand> {
    let Some(dir) = commands_dir() else {
        return Vec::new();
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let mut commands = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        match parse_command_file(&path) {
            Ok(cmd) => commands.push(cmd),
            Err(e) => {
                tracing::warn!("skipping command file {}: {e}", path.display());
            }
        }
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands
}

fn commands_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".xiaoo").join("command"))
}

fn parse_command_file(path: &PathBuf) -> Result<ExternalCommand, String> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "cannot derive command name from filename".to_string())?;

    let content = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;

    let (frontmatter, body) = split_frontmatter(&content);
    let description = frontmatter
        .and_then(|fm| extract_field(fm, "description"))
        .unwrap_or_default();

    Ok(ExternalCommand {
        name,
        description,
        body: body.trim().to_string(),
    })
}

/// Split markdown content into optional frontmatter and body.
/// Frontmatter is delimited by `---` lines at the start.
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }

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

/// Extract a simple `key: value` field from YAML-like frontmatter text.
fn extract_field<'a>(frontmatter: &'a str, key: &str) -> Option<String> {
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            if k.trim() == key {
                let v = v.trim();
                let v = v.trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_frontmatter_basic() {
        let content = "---\ndescription: hello world\n---\nBody here";
        let (fm, body) = split_frontmatter(content);
        assert_eq!(fm, Some("description: hello world"));
        assert_eq!(body, "Body here");
    }

    #[test]
    fn split_frontmatter_no_yaml() {
        let content = "Just plain text";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn extract_field_basic() {
        let fm = "description: Agent project development\ndisable-model-invocation: true";
        assert_eq!(
            extract_field(fm, "description"),
            Some("Agent project development".to_string())
        );
        assert_eq!(
            extract_field(fm, "disable-model-invocation"),
            Some("true".to_string())
        );
        assert_eq!(extract_field(fm, "missing"), None);
    }

    #[test]
    fn extract_field_quoted() {
        let fm = "description: \"quoted value\"";
        assert_eq!(
            extract_field(fm, "description"),
            Some("quoted value".to_string())
        );
    }
}
