use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolRegistry, ToolSpecView};
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::ToolFilterImpl;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolSpecSnapshot {
    pub id: ToolId,
    pub name: ToolName,
    pub description: String,
    pub input_schema: InputSchemaRef,
    pub output_contract: OutputContract,
    pub effect_profile: EffectProfile,
}

impl From<&dyn ToolSpecView> for ToolSpecSnapshot {
    fn from(value: &dyn ToolSpecView) -> Self {
        Self {
            id: value.id().clone(),
            name: value.name().clone(),
            description: value.description().to_string(),
            input_schema: value.input_schema().clone(),
            output_contract: value.output_contract().clone(),
            effect_profile: value.effect_profile().clone(),
        }
    }
}

impl ToolSpecView for ToolSpecSnapshot {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}

pub fn snapshot_tool_specs<'a, I>(specs: I) -> Vec<ToolSpecSnapshot>
where
    I: IntoIterator<Item = &'a dyn ToolSpecView>,
{
    specs.into_iter().map(ToolSpecSnapshot::from).collect()
}

pub fn tool_specs_from_snapshot(manifest: &[ToolSpecSnapshot]) -> Vec<Arc<dyn ToolSpecView>> {
    manifest
        .iter()
        .cloned()
        .map(|snapshot| Arc::new(snapshot) as Arc<dyn ToolSpecView>)
        .collect()
}

pub fn tool_filter_from_specs(
    visible_specs: &[Arc<dyn ToolSpecView>],
    registry: &dyn ToolRegistry,
) -> Box<dyn ToolFilter> {
    let executors_by_name: HashMap<String, Arc<dyn ToolExecutor>> = visible_specs
        .iter()
        .filter_map(|spec| {
            registry.get_executor(spec.id()).and_then(|executor| {
                let executor_spec = executor.spec();
                if executor_spec.id() == spec.id() && executor_spec.name() == spec.name() {
                    Some((spec.name().0.clone(), executor))
                } else {
                    None
                }
            })
        })
        .collect();

    Box::new(ToolFilterImpl::new(
        visible_specs.to_vec(),
        executors_by_name,
    ))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/tool/framework/registry/manifest_test.rs"]
mod tests;
