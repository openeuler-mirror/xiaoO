use agent_types::common::BuildError;
use agent_types::hooker::HookPointId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HookPointCategory {
    ToolPre,
    ToolPost,
    ToolError,
    LlmPre,
    LlmPost,
    LlmError,
    SessionCreated,
    SessionClosed,
}

pub fn resolve_hook_point_category(
    hook_point: &HookPointId,
) -> Result<HookPointCategory, BuildError> {
    let segments: Vec<_> = hook_point.0.split('.').collect();

    if segments.len() != 4 {
        return Err(BuildError::InvalidConfig {
            message: format!(
                "hook_point must have 4 dot-separated segments: {}",
                hook_point.0
            ),
        });
    }

    let [_agent, action, _detail, stage] = [segments[0], segments[1], segments[2], segments[3]];

    if action.trim().is_empty() || stage.trim().is_empty() {
        return Err(BuildError::InvalidConfig {
            message: format!(
                "hook_point action/stage segments must not be empty: {}",
                hook_point.0
            ),
        });
    }

    match (
        action.to_lowercase().as_str(),
        stage.to_lowercase().as_str(),
    ) {
        ("tool", "pre") => Ok(HookPointCategory::ToolPre),
        ("tool", "post") => Ok(HookPointCategory::ToolPost),
        ("tool", "error") => Ok(HookPointCategory::ToolError),
        ("llm", "pre") => Ok(HookPointCategory::LlmPre),
        ("llm", "post") => Ok(HookPointCategory::LlmPost),
        ("llm", "error") => Ok(HookPointCategory::LlmError),
        ("session", "created") => Ok(HookPointCategory::SessionCreated),
        ("session", "closed") => Ok(HookPointCategory::SessionClosed),
        (action, stage) => Err(BuildError::InvalidConfig {
            message: format!(
                "unsupported hook_point category for action='{}' stage='{}': {}",
                action, stage, hook_point.0
            ),
        }),
    }
}
