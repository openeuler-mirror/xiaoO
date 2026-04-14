use crate::tool::executor::ToolExecutor;
use crate::tool::spec::ToolSpecView;
use std::sync::Arc;

pub struct DiscoveredTool {
    pub spec: Arc<dyn ToolSpecView>,
    pub executor: Arc<dyn ToolExecutor>,
}

pub trait ToolSource: Send + Sync {
    fn discover(&self) -> Vec<DiscoveredTool>;
}
