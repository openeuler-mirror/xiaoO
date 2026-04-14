use agent_types::common::BuildError;
use serde_json::Value;

use super::backend::BACKEND_TYPE_MOIRAI_SQLITE;
pub(crate) const DEFAULT_DB_PATH: &str = "./.clawtrace/traces.db";

#[derive(Debug, Clone)]
pub(crate) struct TraceRecorderConfig {
    pub(crate) storage_backend: String,
    pub(crate) db_path: Option<String>,
    pub(crate) agent_id: Option<String>,
}

impl Default for TraceRecorderConfig {
    fn default() -> Self {
        Self {
            storage_backend: BACKEND_TYPE_MOIRAI_SQLITE.to_string(),
            db_path: Some(DEFAULT_DB_PATH.to_string()),
            agent_id: None,
        }
    }
}

impl TraceRecorderConfig {
    pub(crate) fn from_json(config: Value) -> Result<Self, BuildError> {
        match config {
            Value::Null => Ok(Self::default()),
            Value::Object(map) => {
                let storage_backend = match map.get("storage_backend") {
                    None => BACKEND_TYPE_MOIRAI_SQLITE.to_string(),
                    Some(Value::String(value)) => value.clone(),
                    Some(_) => {
                        return Err(BuildError::InvalidConfig {
                            message: "storage_backend must be a string".to_string(),
                        });
                    }
                };

                let db_path = parse_optional_string(&map, "db_path")?
                    .or_else(|| Some(DEFAULT_DB_PATH.to_string()));
                let agent_id = parse_optional_string(&map, "agent_id")?;

                Ok(Self {
                    storage_backend,
                    db_path,
                    agent_id,
                })
            }
            _ => Err(BuildError::InvalidConfig {
                message: "trace recorder config must be a JSON object or null".to_string(),
            }),
        }
    }
}

fn parse_optional_string(
    map: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, BuildError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(BuildError::InvalidConfig {
            message: format!("{key} must be a string when provided"),
        }),
    }
}
