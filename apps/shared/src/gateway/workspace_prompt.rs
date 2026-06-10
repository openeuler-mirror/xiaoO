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
