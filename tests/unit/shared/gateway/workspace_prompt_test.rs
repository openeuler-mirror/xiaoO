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
