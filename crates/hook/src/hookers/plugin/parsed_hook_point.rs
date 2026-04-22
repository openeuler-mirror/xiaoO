use agent_types::common::{AgentId, BuildError};
use agent_types::hook::HookPointId;

pub(crate) struct ParsedPluginHookPoint {
    pub(crate) agent: AgentId,
    pub(crate) action: PluginHookAction,
    pub(crate) detail: PluginHookDetail,
    pub(crate) stage: PluginHookStage,
}

pub(crate) struct PluginHookAction(pub(crate) String);

#[allow(dead_code)]
pub(crate) struct PluginHookDetail(pub(crate) String);

pub(crate) struct PluginHookStage(pub(crate) String);

// Splits hook_point into agent/action/detail/stage for routing.
pub(crate) fn parse_plugin_hook_point(
    hook_point: &HookPointId,
) -> Result<ParsedPluginHookPoint, BuildError> {
    let segments: Vec<_> = hook_point.0.split('.').collect();

    if segments.len() != 4 {
        return Err(BuildError::InvalidConfig {
            message: format!(
                "plugin hooker hook_point must have 4 dot-separated segments: {}",
                hook_point.0
            ),
        });
    }

    let [agent, action, detail, stage] = [segments[0], segments[1], segments[2], segments[3]];

    if agent.trim().is_empty()
        || action.trim().is_empty()
        || detail.trim().is_empty()
        || stage.trim().is_empty()
    {
        return Err(BuildError::InvalidConfig {
            message: format!(
                "plugin hooker hook_point segments must not be empty: {}",
                hook_point.0
            ),
        });
    }

    Ok(ParsedPluginHookPoint {
        agent: AgentId(agent.to_string()),
        action: PluginHookAction(action.to_lowercase()),
        detail: PluginHookDetail(detail.to_string()),
        stage: PluginHookStage(stage.to_lowercase()),
    })
}
