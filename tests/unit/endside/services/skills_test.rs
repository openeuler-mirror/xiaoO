use super::render_skills_overview_with_config;
use tempfile::TempDir;
use xiaoo_shared::skills_support::SkillsConfig;

fn skills_config_with_dirs(dirs: Vec<std::path::PathBuf>) -> SkillsConfig {
    SkillsConfig {
        skills_dirs: dirs,
        ..SkillsConfig::default()
    }
}

#[test]
fn render_skills_overview_lists_loaded_skills() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let skills_root = temp_dir.path().join("skills");
    let review_dir = skills_root.join("reviewer");
    std::fs::create_dir_all(&review_dir).expect("create skill dir");
    std::fs::write(
        review_dir.join("SKILL.md"),
        "---\ndescription: 审查当前改动\n---\nReview the current patch.",
    )
    .expect("write skill file");

    let config = skills_config_with_dirs(vec![skills_root]);
    let rendered = render_skills_overview_with_config(&config);
    assert!(rendered.contains("当前可用 skills（1）:"));
    assert!(rendered.contains("- reviewer: 审查当前改动"));
}

#[test]
fn render_skills_overview_shows_scanned_dirs_when_empty() {
    let temp_dir = TempDir::new().expect("create temp dir");
    let empty_root = temp_dir.path().join("empty-skills");
    std::fs::create_dir_all(&empty_root).expect("create empty skill root");

    let config = skills_config_with_dirs(vec![empty_root.clone()]);
    let rendered = render_skills_overview_with_config(&config);
    assert!(rendered.contains("当前未发现可用的 skills。"));
    assert!(rendered.contains(&empty_root.display().to_string()));
}
