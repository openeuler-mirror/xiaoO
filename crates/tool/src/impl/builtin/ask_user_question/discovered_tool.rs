use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::AskUserQuestionExecutor;
use super::spec::AskUserQuestionToolSpec;

pub(crate) fn discover_ask_user_question() -> DiscoveredTool {
    let spec = Arc::new(AskUserQuestionToolSpec::new());
    let executor = AskUserQuestionExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
