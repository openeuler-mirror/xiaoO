use crate::gateway::session_record::SubagentRoleRecord;
use std::collections::BTreeMap;

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
        "\n\n## Subagent Delegation Rules\n\n\
        When handling user requests, you MUST check if there is a suitable predefined subagent role available. \
        If a predefined subagent role's description matches the user's request scenario, you MUST use spawn_subagent \
        with that subagent_role_id to delegate the task, instead of executing it yourself.\n\n\
        **Critical**: spawn_subagent returns an agent_id but does NOT wait for completion. The subagent runs asynchronously. \
        You MUST call join_subagent with the returned agent_id at an appropriate point in your workflow to wait for the \
        subagent to finish and receive its results. Your delegated task is NOT complete until you call join_subagent and \
        process the returned results.\n\n\
        Available predefined subagent roles:\n{}\n\n\
        **Important**: Do NOT attempt to complete tasks yourself when a matching subagent role exists. \
        Delegation ensures specialized handling and better results.",
        roles_list
    ))
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
        assert!(rules.contains("## Subagent Delegation Rules"));
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

        assert!(rules.starts_with("\n\n##"));
        assert!(rules.contains("MUST check"));
        assert!(rules.contains("**Critical**"));
        assert!(rules.contains("Available predefined subagent roles"));
        assert!(rules.contains("**Important**"));
    }
}
