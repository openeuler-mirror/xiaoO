use agent_contracts::Hooker;

use super::helloworld_closed::BuiltinSessionClosedHooker;
use super::helloworld_created::BuiltinSessionCreatedHooker;

pub(crate) fn build_builtin_session_hookers() -> Vec<Box<dyn Hooker>> {
    vec![
        Box::new(BuiltinSessionCreatedHooker::new()),
        Box::new(BuiltinSessionClosedHooker::new()),
    ]
}
