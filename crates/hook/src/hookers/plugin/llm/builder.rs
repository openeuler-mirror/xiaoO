use agent_contracts::Hooker;
use agent_types::common::BuildError;

use super::super::definition::PluginHookerDefinition;
use super::super::parsed_hook_point::ParsedPluginHookPoint;
use super::adaptor::PluginLlmHookerAdaptor;

pub(crate) fn build_plugin_llm_hooker(
    definition: PluginHookerDefinition,
    parsed_hook_point: ParsedPluginHookPoint,
) -> Result<Box<dyn Hooker>, BuildError> {
    let _agent = &parsed_hook_point.agent;
    let _detail = &parsed_hook_point.detail;

    match parsed_hook_point.stage.0.as_str() {
        "pre" | "post" | "error" => {
            let hooker = PluginLlmHookerAdaptor::new(
                definition.id,
                definition.hook_point,
                definition.command,
                definition.definition,
            );
            Ok(Box::new(hooker))
        }
        stage => Err(BuildError::InvalidConfig {
            message: format!("unsupported llm plugin hooker stage: {}", stage),
        }),
    }
}
