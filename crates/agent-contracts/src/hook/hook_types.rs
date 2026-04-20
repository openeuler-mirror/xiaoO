pub trait HookInput {
    fn as_json(&self) -> Result<serde_json::Value, agent_types::common::BuildError>;
}

pub trait HookResult: Sized {
    type Input: HookInput;

    fn from_json_and_input(
        json: serde_json::Value,
        input: &Self::Input,
    ) -> Result<Self, agent_types::common::BuildError>;
}
