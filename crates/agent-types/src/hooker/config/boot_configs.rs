use crate::common::HookerId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum HookerDefaultMode {
    #[default]
    #[serde(alias = "all", alias = "ALL")]
    All,
    #[serde(alias = "none", alias = "NONE")]
    None,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HookerRegistryConfig {
    #[serde(default)]
    pub default: HookerDefaultMode,
    #[serde(default)]
    pub enabled: Vec<HookerId>,
    #[serde(default)]
    pub disabled: Vec<HookerId>,
    #[serde(default)]
    pub policies: HashMap<HookerId, serde_json::Value>,
    #[serde(default)]
    pub plugins: Vec<String>,
}
