use agent_types::common::BuildError;
use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};
use hook::framework::HookerRegistryBuilderImpl;
use hook::{resolve_hook_point_category, HookPointCategory, HookerRegistryBuilder};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HookCatalogReport {
    pub schema_version: u32,
    pub default_mode: &'static str,
    pub max_prompt_chain_depth: usize,
    pub plugins: Vec<HookPluginSummary>,
    pub hooks: Vec<HookSummary>,
}

#[derive(Debug, Serialize)]
pub struct HookPluginSummary {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Serialize)]
pub struct HookSummary {
    pub id: String,
    pub hook_point: String,
    pub category: &'static str,
    pub enabled: bool,
    pub has_policy: bool,
}

pub fn hook_catalog(config: &HookerRegistryConfig) -> Result<HookCatalogReport, BuildError> {
    let registry = HookerRegistryBuilderImpl::new()
        .with_config(config.clone())
        .build()?;
    let mut hooks = registry
        .list()
        .into_iter()
        .map(|hooker| {
            let id = hooker.id();
            Ok(HookSummary {
                id: id.to_string(),
                hook_point: hooker.hook_point().0.clone(),
                category: category_name(resolve_hook_point_category(hooker.hook_point())?),
                enabled: registry.is_enabled(id),
                has_policy: registry.policy_for(id).is_some(),
            })
        })
        .collect::<Result<Vec<_>, BuildError>>()?;
    hooks.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(HookCatalogReport {
        schema_version: 1,
        default_mode: match config.default {
            HookerDefaultMode::All => "all",
            HookerDefaultMode::None => "none",
        },
        max_prompt_chain_depth: config.max_prompt_chain_depth,
        plugins: config
            .plugins
            .iter()
            .map(|path| HookPluginSummary {
                path: path.clone(),
                exists: std::path::Path::new(path).is_file(),
            })
            .collect(),
        hooks,
    })
}

fn category_name(category: HookPointCategory) -> &'static str {
    match category {
        HookPointCategory::ToolPre => "tool_pre",
        HookPointCategory::ToolPost => "tool_post",
        HookPointCategory::ToolError => "tool_error",
        HookPointCategory::LlmPre => "llm_pre",
        HookPointCategory::LlmPost => "llm_post",
        HookPointCategory::LlmError => "llm_error",
        HookPointCategory::SessionCreated => "session_created",
        HookPointCategory::SessionClosed => "session_closed",
        HookPointCategory::SessionState => "session_state",
        HookPointCategory::ChatSystemTransform => "chat_system_transform",
        HookPointCategory::ChatMessage => "chat_message",
        HookPointCategory::CommandExecuteBefore => "command_execute_before",
    }
}

#[cfg(test)]
mod tests {
    use super::hook_catalog;
    use agent_types::common::HookerId;
    use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};

    #[test]
    fn reports_registered_hooks_and_effective_state() {
        let mut config = HookerRegistryConfig {
            default: HookerDefaultMode::None,
            ..HookerRegistryConfig::default()
        };
        config
            .enabled
            .push(HookerId("builtin_session_created_hooker".to_string()));
        config.policies.insert(
            HookerId("builtin_session_created_hooker".to_string()),
            serde_json::json!({ "message": "ready" }),
        );

        let report = hook_catalog(&config).expect("hook catalog");
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.default_mode, "none");
        let hook = report
            .hooks
            .iter()
            .find(|hook| hook.id == "builtin_session_created_hooker")
            .expect("session hook");
        assert!(hook.enabled);
        assert!(hook.has_policy);
        assert_eq!(hook.category, "session_created");
    }
}
