use agent_contracts::Hooker;

use super::llm::build_builtin_llm_hookers;
use super::session::build_builtin_session_hookers;
use super::tool::build_builtin_tool_hookers;

pub(crate) fn build_builtin_hookers() -> Vec<Box<dyn Hooker>> {
    let mut hookers = Vec::new();
    hookers.extend(build_builtin_tool_hookers());
    hookers.extend(build_builtin_llm_hookers());
    hookers.extend(build_builtin_session_hookers());
    hookers
}
