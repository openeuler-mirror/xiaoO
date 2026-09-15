use std::path::{Path, PathBuf};

#[allow(dead_code)]
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
#[path = "../../../../tests/unit/tool/impl/path_resolver_test.rs"]
mod tests;
