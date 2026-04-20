use agent_types::common::{BuildError, HookerId};
use agent_types::hook::HookPointId;

pub(crate) struct PluginHookerDefinition {
    pub(crate) id: HookerId,
    pub(crate) hook_point: HookPointId,
    pub(crate) command: String,
    pub(crate) definition: serde_json::Value,
}

// Shared JSON declaration parser for plugin hookers.
pub(crate) fn parse_plugin_hooker_definition_from_json(
    json: &str,
) -> Result<PluginHookerDefinition, BuildError> {
    let definition: serde_json::Value =
        serde_json::from_str(json).map_err(|error| BuildError::InvalidConfig {
            message: format!("invalid plugin hooker json: {error}"),
        })?;

    let id = read_required_string_field(&definition, "id")?;
    let hook_point = read_required_string_field(&definition, "hook_point")?;
    let command = read_required_string_field(&definition, "command")?;

    if id.trim().is_empty() {
        return Err(BuildError::InvalidConfig {
            message: "plugin hooker id must not be empty".to_string(),
        });
    }

    if hook_point.trim().is_empty() {
        return Err(BuildError::InvalidConfig {
            message: "plugin hooker hook_point must not be empty".to_string(),
        });
    }

    if command.trim().is_empty() {
        return Err(BuildError::InvalidConfig {
            message: "plugin hooker command must not be empty".to_string(),
        });
    }

    Ok(PluginHookerDefinition {
        id: HookerId(id.to_string()),
        hook_point: HookPointId(hook_point.to_string()),
        command: command.to_string(),
        definition,
    })
}

fn read_required_string_field<'a>(
    definition: &'a serde_json::Value,
    field_name: &str,
) -> Result<&'a str, BuildError> {
    definition
        .get(field_name)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| BuildError::InvalidConfig {
            message: format!(
                "plugin hooker json must contain string field '{}'",
                field_name
            ),
        })
}
