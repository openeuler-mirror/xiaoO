use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolRegistry, ToolSpecView};
use agent_types::common::ids::{AgentId, ToolId};
use agent_types::tool::ToolVisibilityConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::ToolFilterImpl;

pub struct ToolRegistryImpl {
    executors: HashMap<ToolId, Arc<dyn ToolExecutor>>,
    specs: HashMap<ToolId, Arc<dyn ToolSpecView>>,
    visibility_config: ToolVisibilityConfig,
}

impl ToolRegistryImpl {
    pub fn new(
        executors: HashMap<ToolId, Arc<dyn ToolExecutor>>,
        specs: HashMap<ToolId, Arc<dyn ToolSpecView>>,
        visibility_config: ToolVisibilityConfig,
    ) -> Self {
        Self {
            executors,
            specs,
            visibility_config,
        }
    }

    fn resolve_visible_specs_for_agent(&self, agent_id: &AgentId) -> Vec<Arc<dyn ToolSpecView>> {
        let Some(allowed_tool_names) = self.visibility_config.per_agent_allowed_tools.get(agent_id)
        else {
            return Vec::new();
        };

        let allowed_tool_names: HashSet<_> = allowed_tool_names
            .iter()
            .map(|tool_name| tool_name.0.as_str())
            .collect();

        self.specs
            .values()
            .filter(|spec| allowed_tool_names.contains(spec.name().0.as_str()))
            .map(Arc::clone)
            .collect()
    }

    fn resolve_visible_executors_by_name(
        &self,
        visible_specs: &[Arc<dyn ToolSpecView>],
    ) -> HashMap<String, Arc<dyn ToolExecutor>> {
        visible_specs
            .iter()
            .filter_map(|spec| {
                self.executors
                    .get(spec.id())
                    .map(|executor| (spec.name().0.clone(), Arc::clone(executor)))
            })
            .collect()
    }
}

impl ToolRegistry for ToolRegistryImpl {
    fn get_executor(&self, id: &ToolId) -> Option<Arc<dyn ToolExecutor>> {
        self.executors.get(id).map(Arc::clone)
    }

    fn get_spec(&self, id: &ToolId) -> Option<&dyn ToolSpecView> {
        self.specs.get(id).map(|spec| spec.as_ref())
    }

    fn list_specs(&self) -> Vec<&dyn ToolSpecView> {
        self.specs
            .values()
            .map(|spec| spec.as_ref() as &dyn ToolSpecView)
            .collect()
    }

    fn filter_for(&self, agent_id: &AgentId) -> Box<dyn ToolFilter> {
        let visible_specs = self.resolve_visible_specs_for_agent(agent_id);
        let executors_by_name = self.resolve_visible_executors_by_name(&visible_specs);
        Box::new(ToolFilterImpl::new(visible_specs, executors_by_name))
    }
}
