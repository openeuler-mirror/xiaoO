use crate::common::HookerId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum HookerDefaultMode {
    #[default]
    All,
    None,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HookerRegistryConfig {
    pub default: HookerDefaultMode,
    pub enabled: Vec<HookerId>,
    pub disabled: Vec<HookerId>,
    pub policies: HashMap<HookerId, serde_json::Value>,
    pub plugins: Vec<String>,
}
