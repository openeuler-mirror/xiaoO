use agent_contracts::Hooker;
use agent_types::common::BuildError;

use super::super::definition::PluginHookerDefinition;
use super::super::parsed_hook_point::ParsedPluginHookPoint;
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
