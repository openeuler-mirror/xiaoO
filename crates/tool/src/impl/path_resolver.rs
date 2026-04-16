use agent_contracts::runtime::runtime_view::RuntimeView;
use std::path::{Path, PathBuf};

pub fn runtime_workspace_root(runtime: &dyn RuntimeView) -> &Path {
    runtime.agent_context().workspace().root.as_path()
}

pub fn expand_path_from_base(path: &str, base_dir: &Path) -> String {
    let path = path.trim();

    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return home;
        }
    }

    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }

    let candidate = PathBuf::from(path);
    if candidate.is_relative() {
        return base_dir.join(candidate).to_string_lossy().into_owned();
    }

    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::expand_path_from_base;
    use std::path::Path;

    #[test]
    fn expands_relative_paths_from_workspace_root() {
        let expanded = expand_path_from_base("src/main.rs", Path::new("/tmp/workspace"));
        assert_eq!(expanded, "/tmp/workspace/src/main.rs");
    }

    #[test]
    fn keeps_absolute_paths_unchanged() {
        let expanded = expand_path_from_base("/tmp/workspace/src/main.rs", Path::new("/ignored"));
        assert_eq!(expanded, "/tmp/workspace/src/main.rs");
    }
}
