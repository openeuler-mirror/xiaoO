use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

pub trait ToolSpecView: Send + Sync {
    fn id(&self) -> &ToolId;
    fn name(&self) -> &ToolName;
    fn description(&self) -> &str;
    fn input_schema(&self) -> &InputSchemaRef;
    fn output_contract(&self) -> &OutputContract;
    fn effect_profile(&self) -> &EffectProfile;
}
