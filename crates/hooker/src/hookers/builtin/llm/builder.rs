use agent_contracts::Hooker;

use super::helloworld_post::HelloWorldLlmPostHooker;
use super::helloworld_pre::HelloWorldLlmPreHooker;

pub(crate) fn build_builtin_llm_hookers() -> Vec<Box<dyn Hooker>> {
    vec![
        Box::new(HelloWorldLlmPreHooker::new()),
        Box::new(HelloWorldLlmPostHooker::new()),
    ]
}
