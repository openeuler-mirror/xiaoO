use agent_types::common::BuildError;
use agent_types::hook::HookPointId;

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
    SessionState,
    ChatSystemTransform,
    ChatMessage,
    CommandExecuteBefore,
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
        ("session", "state") => Ok(HookPointCategory::SessionState),
        // Chat-level hooks. The `detail` segment is free-form (e.g.
        // `system` / `message` / `command`) and disambiguated by `stage`.
        ("chat", "transform") => Ok(HookPointCategory::ChatSystemTransform),
        ("chat", "received") => Ok(HookPointCategory::ChatMessage),
        ("chat", "before") => Ok(HookPointCategory::CommandExecuteBefore),
        (action, stage) => Err(BuildError::InvalidConfig {
            message: format!(
                "unsupported hook_point category for action='{}' stage='{}': {}",
                action, stage, hook_point.0
            ),
        }),
    }
}

/// Escape a free-form hook point segment (agent id, tool name) so embedding
/// it in a dot-separated hook point id cannot break the 4-segment structure:
/// `.` becomes `%2E` and `%` becomes `%25` (in that order, keeping the map
/// injective). Inputs without dots or percent signs map to themselves, so
/// already well-formed hook points keep their exact spelling and existing
/// registrations stay valid. Without this, a tool name like `plugin.tool`
/// yields a 5-segment point that no 4-segment pattern (e.g. `*.Tool.*.pre`)
/// can ever match, silently excluding such tools from wildcard hookers.
pub fn escape_hook_point_segment(segment: &str) -> String {
    segment.replace('%', "%25").replace('.', "%2E")
}

#[cfg(test)]
#[path = "../../../../tests/unit/hook/hookers/hook_point_category_test.rs"]
mod tests;
