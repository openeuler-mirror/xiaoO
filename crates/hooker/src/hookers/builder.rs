use agent_contracts::Hooker;
use agent_types::common::BuildError;

use super::builtin::build_builtin_hookers;
#[cfg(feature = "plugin_hook")]
use super::plugin::build_plugin_hookers;

pub(crate) fn build_hookers(
    _plugin_tool_hooker_jsons: &[String],
) -> Result<Vec<Box<dyn Hooker>>, BuildError> {
    let mut hookers = Vec::new();
    hookers.extend(build_builtin_hookers());
    #[cfg(feature = "plugin_hook")]
    hookers.extend(build_plugin_hookers(_plugin_tool_hooker_jsons)?);
    Ok(hookers)
}
