use std::path::{Path, PathBuf};

const AGENTS_FILE_NAME: &str = "AGENTS.md";
// Keep these markers in sync with crates/prompt/src/compose.rs.
const WORKSPACE_PROMPT_MARKER_BEGIN: &str = "<xiaoo_workspace_prompt>";
const WORKSPACE_PROMPT_MARKER_END: &str = "</xiaoo_workspace_prompt>";

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspacePromptFile {
    path: PathBuf,
    content: String,
}

pub fn compose_workspace_system_prompt(base_prompt: &str, workspace_root: &Path) -> String {
    let base_prompt = base_prompt.trim();
    let prompt_files = discover_workspace_prompt_files(workspace_root);

    if prompt_files.is_empty() {
        return base_prompt.to_string();
    }

    let mut sections = Vec::new();
    if !base_prompt.is_empty() {
        sections.push(base_prompt.to_string());
    }
    sections.push(format!(
        "{WORKSPACE_PROMPT_MARKER_BEGIN}\n{}\n{WORKSPACE_PROMPT_MARKER_END}",
        render_workspace_prompt_section(&prompt_files)
    ));

    sections.join("\n\n")
}

fn discover_workspace_prompt_files(workspace_root: &Path) -> Vec<WorkspacePromptFile> {
    let root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let directories = workspace_prompt_directories(&root);

    let mut prompt_files = Vec::new();

    for dir in directories {
        let prompt_path = dir.join(AGENTS_FILE_NAME);
        if !prompt_path.is_file() {
            continue;
        }

        match std::fs::read_to_string(&prompt_path) {
            Ok(content) => {
                let content = content.trim();
                if content.is_empty() {
                    continue;
                }
                prompt_files.push(WorkspacePromptFile {
                    path: prompt_path,
                    content: content.to_string(),
                });
            }
            Err(error) => {
                tracing::warn!(
                    path = %prompt_path.display(),
                    %error,
                    "failed to read AGENTS.md; skipping"
                );
            }
        }
    }

    prompt_files
}

fn workspace_prompt_directories(workspace_root: &Path) -> Vec<PathBuf> {
    let mut directories = vec![workspace_root.to_path_buf()];
    let mut current = workspace_root;
    let mut found_repo_root = current.join(".git").exists();

    while !found_repo_root {
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
        directories.push(current.to_path_buf());
        found_repo_root = current.join(".git").exists();
    }

    if found_repo_root {
        directories.reverse();
        directories
    } else {
        vec![workspace_root.to_path_buf()]
    }
}

fn render_workspace_prompt_section(prompt_files: &[WorkspacePromptFile]) -> String {
    let mut section = String::from(
        "## Workspace Instructions\n\
The following instructions were loaded from AGENTS.md files found in the current workspace \
directory and applicable parent directories. Later files are more specific and take precedence.\n",
    );

    for prompt_file in prompt_files {
        section.push_str("\n### ");
        section.push_str(&prompt_file.path.display().to_string());
        section.push('\n');
        section.push_str(prompt_file.content.trim());
        section.push('\n');
    }

    section.trim_end().to_string()
}

const REPO_MAP_MAX_FILES: usize = 50;
const REPO_MAP_MAX_SIGS_PER_FILE: usize = 8;
const REPO_MAP_MAX_BYTES: usize = 6000;
const REPO_MAP_MAX_DEPTH: usize = 3;
const REPO_MAP_MAX_VISIT: usize = 5000;
const REPO_MAP_SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "__pycache__",
    "dist",
    "build",
    "vendor",
];

fn repo_map_is_source(name: &str) -> bool {
    const EXTS: &[&str] = &[
        "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "rb", "c", "cc", "cpp", "h", "hpp",
        "cs", "php", "kt", "swift", "scala", "sh", "lua", "ex", "exs",
    ];
    name.rsplit('.')
        .next()
        .map(|e| EXTS.contains(&e))
        .unwrap_or(false)
}

fn repo_map_collect(dir: &Path, out: &mut Vec<PathBuf>, visited: &mut usize, depth: usize) {
    if depth > REPO_MAP_MAX_DEPTH || *visited >= REPO_MAP_MAX_VISIT {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        *visited += 1;
        if *visited >= REPO_MAP_MAX_VISIT {
            break;
        }
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if name.starts_with('.') || REPO_MAP_SKIP_DIRS.contains(&name) {
                continue;
            }
            subdirs.push(path);
        } else if file_type.is_file() && repo_map_is_source(name) {
            out.push(path);
        }
    }
    for sub in subdirs {
        repo_map_collect(&sub, out, visited, depth + 1);
    }
}

fn repo_map_signatures(path: &Path) -> Vec<String> {
    const KW: &[&str] = &[
        "pub fn ",
        "pub async fn ",
        "async fn ",
        "fn ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "impl ",
        "def ",
        "async def ",
        "class ",
        "func ",
        "function ",
        "export function ",
        "export class ",
        "export const ",
        "export default ",
        "interface ",
        "type ",
    ];
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() <= 512 * 1024 => {}
        _ => return Vec::new(),
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut sigs = Vec::new();
    for line in content.lines().take(3000) {
        let trimmed = line.trim_start();
        if KW.iter().any(|kw| trimmed.starts_with(kw)) {
            let mut sig: String = trimmed.chars().take(110).collect();
            if let Some(idx) = sig.find(|c| c == '{' || c == ';' || c == '=') {
                sig.truncate(idx);
            }
            let sig = sig.trim_end().to_string();
            if !sig.is_empty() {
                sigs.push(sig);
            }
            if sigs.len() >= REPO_MAP_MAX_SIGS_PER_FILE {
                break;
            }
        }
    }
    sigs
}

pub fn compose_repo_map(workspace_root: &Path) -> Option<String> {
    let root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let mut files = Vec::new();
    let mut visited = 0usize;
    repo_map_collect(&root, &mut files, &mut visited, 0);
    if files.is_empty() {
        return None;
    }
    files.sort();
    files.truncate(REPO_MAP_MAX_FILES);
    let mut out = String::from(
        "## Repository map\nStatic overview of source files and their top-level definitions \
(not exhaustive; use glob/grep for full detail):\n",
    );
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        out.push('\n');
        out.push_str(&rel);
        for sig in repo_map_signatures(file) {
            out.push_str("\n  ");
            out.push_str(&sig);
        }
        if out.len() >= REPO_MAP_MAX_BYTES {
            out.push_str("\n…[repo map truncated]…");
            break;
        }
    }
    Some(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::compose_workspace_system_prompt;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn compose_workspace_system_prompt_loads_agents_from_workspace_ancestors() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path();
        let nested = repo_root.join("apps").join("tui");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(repo_root.join(".git")).unwrap();
        fs::write(repo_root.join("AGENTS.md"), "root rules").unwrap();
        fs::write(repo_root.join("apps").join("AGENTS.md"), "app rules").unwrap();

        let prompt = compose_workspace_system_prompt("base rules", &nested);

        assert!(prompt.starts_with("base rules"));
        assert!(prompt.contains("Workspace Instructions"));
        assert!(prompt.contains(&repo_root.join("AGENTS.md").display().to_string()));
        assert!(prompt.contains(
            &repo_root
                .join("apps")
                .join("AGENTS.md")
                .display()
                .to_string()
        ));
        assert!(prompt.find("root rules").unwrap() < prompt.find("app rules").unwrap());
    }

    #[test]
    fn compose_workspace_system_prompt_keeps_base_prompt_when_no_agents_exist() {
        let temp = tempdir().unwrap();

        let prompt = compose_workspace_system_prompt("base rules", temp.path());

        assert_eq!(prompt, "base rules");
    }
}
