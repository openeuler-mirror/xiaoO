use agent_contracts::Hooker;

use super::helloworld_pre::BuiltinHelloWorldPreHooker;

pub(crate) fn build_builtin_tool_hookers() -> Vec<Box<dyn Hooker>> {
    vec![Box::new(BuiltinHelloWorldPreHooker::new())]
}
