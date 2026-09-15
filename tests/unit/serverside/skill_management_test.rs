use super::skill_catalog;
use crate::daemon_config::DaemonConfig;
use tempfile::TempDir;

#[test]
fn reports_effective_skill_metadata_and_directories() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let skill_dir = temp.path().join(".xiaoo/skills/reviewer");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: reviewer\ndescription: Review code\nuser_invocable: false\n---\nReview carefully.\n",
    )
    .expect("skill");
    std::fs::write(
        &config_path,
        "[llm]\nactive_profile = \"local\"\n\n[llm.profiles.local]\nprovider = \"ollama\"\nmodel = \"qwen3\"\n\n[skills]\ndisabled = [\"reviewer\"]\n",
    )
    .expect("config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");
    let report = skill_catalog(&config, temp.path());

    assert_eq!(report.schema_version, 1);
    assert!(report.directories[0].exists);
    let reviewer = report
        .skills
        .iter()
        .find(|skill| skill.id == "reviewer")
        .expect("reviewer");
    assert_eq!(reviewer.description, "Review code");
    assert!(!reviewer.enabled);
    assert!(!reviewer.user_invocable);
    assert_eq!(reviewer.context, "inline");
}
