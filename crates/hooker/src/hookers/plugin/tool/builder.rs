use agent_contracts::Hooker;
use agent_types::common::BuildError;

use super::super::definition::{parse_plugin_hooker_definition_from_json, PluginHookerDefinition};
use super::super::parsed_hook_point::{parse_plugin_hook_point, ParsedPluginHookPoint};
use super::adaptor::PluginToolHookerAdaptor;

pub(crate) fn build_plugin_tool_hooker(
    definition: PluginHookerDefinition,
    parsed_hook_point: ParsedPluginHookPoint,
) -> Result<Box<dyn Hooker>, BuildError> {
    let _agent = &parsed_hook_point.agent;
    let _detail = &parsed_hook_point.detail;

    // Route by tool hook stage inside the tool family.
    match parsed_hook_point.stage.0.as_str() {
        "pre" | "post" | "error" => {
            let hooker = PluginToolHookerAdaptor::new(
                definition.id,
                definition.hook_point,
                definition.command,
                definition.definition,
            );
            Ok(Box::new(hooker))
        }
        stage => Err(BuildError::InvalidConfig {
            message: format!("unsupported tool plugin hooker stage: {}", stage),
        }),
    }
}

pub fn build_plugin_tool_hooker_adaptor_from_json(
    json: &str,
) -> Result<PluginToolHookerAdaptor, BuildError> {
    let definition = parse_plugin_hooker_definition_from_json(json)?;
    let parsed_hook_point = parse_plugin_hook_point(&definition.hook_point)?;

    if parsed_hook_point.action.0 != "tool" {
        return Err(BuildError::InvalidConfig {
            message: format!(
                "plugin tool hooker builder received non-tool action: {}",
                parsed_hook_point.action.0
            ),
        });
    }

    let _agent = &parsed_hook_point.agent;
    let _detail = &parsed_hook_point.detail;

    match parsed_hook_point.stage.0.as_str() {
        "pre" | "post" | "error" => Ok(PluginToolHookerAdaptor::new(
            definition.id,
            definition.hook_point,
            definition.command,
            definition.definition,
        )),
        stage => Err(BuildError::InvalidConfig {
            message: format!("unsupported tool plugin hooker stage: {}", stage),
        }),
    }
}
