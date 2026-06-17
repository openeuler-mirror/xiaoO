use crate::gateway::session_record::SubagentRoleRecord;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn compose_subagent_delegation_rules(
    subagent_roles: &BTreeMap<String, SubagentRoleRecord>,
) -> Option<String> {
    if subagent_roles.is_empty() {
        return None;
    }

    let roles_list = subagent_roles
        .values()
        .map(|role| format!("- \"{}\": {}", role.role_id, role.description))
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "\n\n## Subagent Delegation\n\n\
        When a predefined subagent role matches the user's request, delegate to it using `spawn_subagent` with `subagent_role_id`. **Workflow**: spawn → wait → `join_subagent` → process results.\n\n\
        **Available Roles**:\n{}\n\n\
        Always delegate to matching roles instead of handling directly.",
        roles_list
    ))
}

pub fn generate_skills_dirs_table(skills_dirs: &[PathBuf]) -> String {
    if skills_dirs.is_empty() {
        return "| Priority | Directory | Purpose |\n|----------|-----------|---------|\n| (none configured) | - | - |".to_string();
    }

    let mut table =
        "| Priority | Directory | Purpose |\n|----------|-----------|---------|\n".to_string();

    let mut config_counter = 0;

    for dir in skills_dirs {
        let dir_str = dir.display().to_string();
        let (priority, purpose) = classify_skill_dir(&dir_str, &mut config_counter);

        table.push_str(&format!("| {} | `{}` | {} |\n", priority, dir_str, purpose));
    }

    table.trim_end().to_string()
}

fn classify_skill_dir(dir: &str, config_counter: &mut usize) -> (String, String) {
    if dir == ".xiaoo/skills" {
        ("Project".to_string(), "Project-specific skills".to_string())
    } else if dir == "/usr/lib/.xiaoo/skills" {
        ("System".to_string(), "Built-in skills".to_string())
    } else if dir.ends_with("/.xiaoo/skills")
        && (dir.starts_with('~') || dir.starts_with("/home/") || dir.starts_with("/root/"))
    {
        ("User".to_string(), "Personal skills".to_string())
    } else {
        *config_counter += 1;
        (
            "Config".to_string(),
            format!("Configured skill dir {}", config_counter),
        )
    }
}

#[cfg(test)]
mod tests {
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
}
