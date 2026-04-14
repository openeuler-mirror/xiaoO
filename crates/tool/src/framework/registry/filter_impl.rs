use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolSpecView};
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolFilterImpl {
    visible_specs: Vec<Arc<dyn ToolSpecView>>,
    specs_by_name: HashMap<String, Arc<dyn ToolSpecView>>,
    executors_by_name: HashMap<String, Arc<dyn ToolExecutor>>,
}

impl ToolFilterImpl {
    pub fn new(
        visible_specs: Vec<Arc<dyn ToolSpecView>>,
        executors_by_name: HashMap<String, Arc<dyn ToolExecutor>>,
    ) -> Self {
        let specs_by_name = visible_specs
            .iter()
            .map(|spec| (spec.name().0.clone(), Arc::clone(spec)))
            .collect();

        Self {
            visible_specs,
            specs_by_name,
            executors_by_name,
        }
    }
}

impl ToolFilter for ToolFilterImpl {
    fn visible_tools(&self) -> Vec<&dyn ToolSpecView> {
        self.visible_specs
            .iter()
            .map(|spec| spec.as_ref() as &dyn ToolSpecView)
            .collect()
    }

    fn allows_tool_name(&self, tool_name: &str) -> bool {
        self.specs_by_name.contains_key(tool_name)
    }

    fn get_spec_for_name(&self, tool_name: &str) -> Option<Arc<dyn ToolSpecView>> {
        self.specs_by_name.get(tool_name).map(Arc::clone)
    }

    fn get_executor_for_name(&self, tool_name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.executors_by_name.get(tool_name).map(Arc::clone)
    }
}
