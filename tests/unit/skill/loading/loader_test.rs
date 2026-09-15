use super::load_skills;
use crate::types::config::SkillsConfig;
use std::collections::HashSet;
use tempfile::TempDir;

#[test]
fn disabled_skills_are_not_loaded() {
    let temp = TempDir::new().expect("tempdir");
    let skill_dir = temp.path().join("skills/reviewer");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: reviewer\ndescription: Review code\n---\nReview carefully.\n",
    )
    .expect("skill");

    let config = SkillsConfig {
        skills_dirs: vec![temp.path().join("skills")],
        disabled: HashSet::from(["reviewer".to_string()]),
        ..SkillsConfig::default()
    };

    assert!(load_skills(&config).is_empty());
}
