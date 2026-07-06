use agent_contracts::Hooker;
use agent_types::common::BuildError;

use super::super::definition::PluginHookerDefinition;
use super::super::parsed_hook_point::ParsedPluginHookPoint;
use super::adaptor::PluginSessionHookerAdaptor;

pub(crate) fn build_plugin_session_hooker(
    definition: PluginHookerDefinition,
    parsed_hook_point: ParsedPluginHookPoint,
) -> Result<Box<dyn Hooker>, BuildError> {
    match parsed_hook_point.stage.0.as_str() {
        "state" => {
            let hooker = PluginSessionHookerAdaptor::new(
                definition.id,
                definition.hook_point,
                definition.command,
                definition.definition,
            );
            Ok(Box::new(hooker))
        }
        stage => Err(BuildError::InvalidConfig {
            message: format!("unsupported session plugin hooker stage: {}", stage),
        }),
    }
}
