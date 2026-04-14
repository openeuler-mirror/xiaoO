use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Clone)]
pub struct PrintHelloWorldToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl PrintHelloWorldToolSpec {
    pub fn new() -> Self {
        Self {
            id: ToolId("builtin_print_hello_world".to_string()),
            name: ToolName("print_hello_world".to_string()),
            description: "A simple tool that prints 'Hello, World!'".to_string(),
            input_schema: InputSchemaRef {
                schema: serde_json::json!({}),
            },
            output_contract: OutputContract {
                description: "A string containing 'Hello, World!'".to_string(),
            },
            effect_profile: EffectProfile::default(),
        }
    }
}

impl ToolSpecView for PrintHelloWorldToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}
