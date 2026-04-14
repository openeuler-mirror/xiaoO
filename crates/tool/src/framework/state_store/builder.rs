use agent_contracts::tool::{ToolStateStore, ToolStateStoreBuilder};
use agent_types::common::BuildError;
use agent_types::tool::ToolStateStoreConfig;
use serde_json::Value;

use super::noop::NoOpToolStateStore;
use super::stdout::PrintStdoutStateStore;

pub struct ToolStateStoreBuilderImpl {
    config: Option<ToolStateStoreConfig>,
}

impl ToolStateStoreBuilderImpl {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn default() -> Self {
        Self {
            config: Some(ToolStateStoreConfig {
                backend: Value::String("stdout".to_string()),
                retention: Value::Null,
            }),
        }
    }
}

impl ToolStateStoreBuilder for ToolStateStoreBuilderImpl {
    fn with_config(mut self, config: ToolStateStoreConfig) -> Self {
        self.config = Some(config);
        self
    }

    fn build(self) -> Result<Box<dyn ToolStateStore>, BuildError> {
        let config = self
            .config
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "tool_state_store.config".to_string(),
            })?;

        let backend = parse_backend_name(&config.backend)?;

        match backend {
            "noop" => Ok(Box::new(NoOpToolStateStore::new())),
            "stdout" => Ok(Box::new(PrintStdoutStateStore::new())),
            other => Err(BuildError::InvalidConfig {
                message: format!(
                    "unsupported tool state store backend '{}'; expected one of: noop, stdout",
                    other
                ),
            }),
        }
    }
}

fn parse_backend_name(backend: &Value) -> Result<&str, BuildError> {
    match backend {
        Value::String(name) => Ok(name.as_str()),
        Value::Object(object) => {
            object
                .get("type")
                .and_then(Value::as_str)
                .ok_or_else(|| BuildError::InvalidConfig {
                    message: "tool state store backend object must contain string field 'type'"
                        .to_string(),
                })
        }
        other => Err(BuildError::InvalidConfig {
            message: format!(
                "tool state store backend must be a string or object, got {}",
                other
            ),
        }),
    }
}
