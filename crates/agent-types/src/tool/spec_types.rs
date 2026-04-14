use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputSchemaRef {
    pub schema: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputContract {
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EffectProfile {
    pub reads_filesystem: bool,
    pub writes_filesystem: bool,
    pub network_access: bool,
    pub side_effects: bool,
}

impl Default for EffectProfile {
    fn default() -> Self {
        Self {
            reads_filesystem: false,
            writes_filesystem: false,
            network_access: false,
            side_effects: false,
        }
    }
}
