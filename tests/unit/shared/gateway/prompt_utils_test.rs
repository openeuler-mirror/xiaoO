use super::*;

fn create_test_role(id: &str, desc: &str) -> SubagentRoleRecord {
    SubagentRoleRecord {
        role_id: id.to_string(),
        description: desc.to_string(),
        prompt: None,
        max_turns: None,
        tools: BTreeMap::new(),
    }
}

#[test]
fn test_empty_roles_returns_none() {
    let roles = BTreeMap::new();
    let result = compose_subagent_delegation_rules(&roles);
    assert!(result.is_none());
}

#[test]
fn test_single_role_formats_correctly() {
    let mut roles = BTreeMap::new();
    roles.insert(
        "code_reviewer".to_string(),
        create_test_role("code_reviewer", "Reviews code quality"),
    );

    let result = compose_subagent_delegation_rules(&roles);
    assert!(result.is_some());

    let rules = result.unwrap();
    assert!(rules.contains("## Subagent Delegation"));
    assert!(rules.contains("- \"code_reviewer\": Reviews code quality"));
    assert!(rules.contains("spawn_subagent"));
    assert!(rules.contains("join_subagent"));
}

#[test]
fn test_multiple_roles_format_list() {
    let mut roles = BTreeMap::new();
    roles.insert(
        "reviewer".to_string(),
        create_test_role("reviewer", "Code reviewer"),
    );
    roles.insert(
        "tester".to_string(),
        create_test_role("tester", "Test writer"),
    );

    let result = compose_subagent_delegation_rules(&roles);
    let rules = result.unwrap();

    assert!(rules.contains("- \"reviewer\": Code reviewer"));
    assert!(rules.contains("- \"tester\": Test writer"));
}

#[test]
fn test_rules_content_structure() {
    let mut roles = BTreeMap::new();
    roles.insert("agent1".to_string(), create_test_role("agent1", "desc1"));

    let rules = compose_subagent_delegation_rules(&roles).unwrap();

    assert!(rules.contains("## Subagent Delegation"));
    assert!(rules.contains("spawn_subagent"));
    assert!(rules.contains("join_subagent"));
    assert!(rules.contains("**Available Roles**"));
}

#[test]
fn test_generate_skills_dirs_table_empty() {
    let dirs: Vec<PathBuf> = Vec::new();
    let table = generate_skills_dirs_table(&dirs);
    assert!(table.contains("(none configured)"));
}

#[test]
fn test_generate_skills_dirs_table_default_four_levels() {
    let dirs = vec![
        PathBuf::from(".xiaoo/skills"),
        PathBuf::from("/home/user/.xiaoo/skills"),
        PathBuf::from("/usr/lib/.xiaoo/skills"),
    ];
    let table = generate_skills_dirs_table(&dirs);
    assert!(table.contains("Project"));
    assert!(table.contains("User"));
    assert!(table.contains("System"));
    assert!(table.contains(".xiaoo/skills"));
    assert!(table.contains("/home/user/.xiaoo/skills"));
    assert!(table.contains("/usr/lib/.xiaoo/skills"));
}

#[test]
fn test_generate_skills_dirs_table_with_config_dirs() {
    let dirs = vec![
        PathBuf::from(".xiaoo/skills"),
        PathBuf::from("/opt/custom/skills"),
        PathBuf::from("/etc/xiaoo/skills"),
        PathBuf::from("/home/user/.xiaoo/skills"),
        PathBuf::from("/usr/lib/.xiaoo/skills"),
    ];
    let table = generate_skills_dirs_table(&dirs);
    assert!(table.contains("Config"));
    assert!(table.contains("Configured skill dir 1"));
    assert!(table.contains("Configured skill dir 2"));
    assert!(table.contains("/opt/custom/skills"));
    assert!(table.contains("/etc/xiaoo/skills"));
    assert!(table.contains("User"));
    assert!(table.contains("/home/user/.xiaoo/skills"));
}

#[test]
fn test_classify_skill_dir_user_variations() {
    let mut counter = 0;

    assert_eq!(
        classify_skill_dir("~/.xiaoo/skills", &mut counter),
        ("User".to_string(), "Personal skills".to_string())
    );

    assert_eq!(
        classify_skill_dir("/home/test/.xiaoo/skills", &mut counter),
        ("User".to_string(), "Personal skills".to_string())
    );

    assert_eq!(
        classify_skill_dir("/root/.xiaoo/skills", &mut counter),
        ("User".to_string(), "Personal skills".to_string())
    );
}
