use std::path::PathBuf;

pub fn first_token_is_dir_command(line: &str) -> bool {
    matches!(line.split_whitespace().next(), Some("/dir") | Some("/cd"))
}

pub fn resolve_dir_command(line: &str) -> Result<PathBuf, String> {
    let mut parts = line.split_whitespace();
    let command = parts.next().unwrap_or_default();
    if !matches!(command, "/dir" | "/cd") {
        return Err("unsupported workspace command".to_string());
    }

    let path = parts
        .next()
        .ok_or_else(|| "workspace command requires a path".to_string())?;
    if parts.next().is_some() {
        return Err("workspace command accepts exactly one path".to_string());
    }

    resolve_workspace_path(path)
}

fn resolve_workspace_path(path_str: &str) -> Result<PathBuf, String> {
    let expanded = expand_home_path(path_str);
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to read current directory: {error}"))?
            .join(expanded)
    };

    let canonical = absolute
        .canonicalize()
        .map_err(|error| format!("invalid workspace path {}: {error}", absolute.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "workspace path is not a directory: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn expand_home_path(value: &str) -> PathBuf {
    if value == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}
