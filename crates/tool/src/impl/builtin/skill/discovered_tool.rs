use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::SkillToolExecutor;
use super::spec::SkillToolSpec;

pub(crate) fn discover_skill() -> DiscoveredTool {
    let spec = Arc::new(SkillToolSpec::new());
    let executor = SkillToolExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
