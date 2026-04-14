use agent_contracts::Hooker;
use agent_types::common::BuildError;

use super::definition::parse_plugin_hooker_definition_from_json;
use super::parsed_hook_point::parse_plugin_hook_point;
use super::tool::build_plugin_tool_hooker;

pub(crate) fn build_plugin_hookers(
    plugin_tool_hooker_jsons: &[String],
) -> Result<Vec<Box<dyn Hooker>>, BuildError> {
    let mut hookers = Vec::new();

    for json in plugin_tool_hooker_jsons {
        let definition = parse_plugin_hooker_definition_from_json(json)?;
        let parsed_hook_point = parse_plugin_hook_point(&definition.hook_point)?;

        // Route by action before entering family-specific builders.
        match parsed_hook_point.action.0.as_str() {
            "tool" => hookers.push(build_plugin_tool_hooker(definition, parsed_hook_point)?),
            action => {
                return Err(BuildError::InvalidConfig {
                    message: format!("unsupported plugin hooker action: {}", action),
                });
            }
        }
    }

    Ok(hookers)
}
