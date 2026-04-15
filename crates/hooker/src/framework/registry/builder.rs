use std::collections::{HashMap, HashSet};
use std::fs;

use crate::framework::HookerRegistryImpl;
use crate::hookers::build_hookers;
use agent_contracts::hooker::{HookerRegistry, HookerRegistryBuilder};
use agent_types::common::BuildError;
use agent_types::hooker::{HookerDefaultMode, HookerRegistryConfig};

pub struct HookerRegistryBuilderImpl {
    // Stores the hooker registry config for later use in build().
    config: HookerRegistryConfig,
}

impl HookerRegistryBuilderImpl {
    pub fn new() -> Self {
        Self {
            config: HookerRegistryConfig::default(),
        }
    }

    fn load_plugin_hooker_jsons(plugin_files: &[String]) -> Result<Vec<String>, BuildError> {
        let mut plugin_hooker_jsons = Vec::new();

        for plugin_file in plugin_files {
            let raw =
                fs::read_to_string(plugin_file).map_err(|error| BuildError::InvalidConfig {
                    message: format!(
                        "failed to read plugin hooker file '{}': {}",
                        plugin_file, error
                    ),
                })?;

            let definitions: Vec<serde_json::Value> =
                serde_json::from_str(&raw).map_err(|error| BuildError::InvalidConfig {
                    message: format!(
                        "plugin hooker file '{}' must be a JSON array: {}",
                        plugin_file, error
                    ),
                })?;

            for definition in definitions {
                plugin_hooker_jsons.push(serde_json::to_string(&definition).map_err(|error| {
                    BuildError::InvalidConfig {
                        message: format!(
                            "failed to normalize plugin hooker entry from '{}': {}",
                            plugin_file, error
                        ),
                    }
                })?);
            }
        }

        Ok(plugin_hooker_jsons)
    }
}

impl HookerRegistryBuilder for HookerRegistryBuilderImpl {
    fn with_config(mut self, config: HookerRegistryConfig) -> Self {
        // Stores the hooker registry config in the builder for later use in build().
        self.config = config;
        self
    }

    fn build(self) -> Result<Box<dyn HookerRegistry>, BuildError> {
        // Uses the stored config and registered hookers to construct the HookerRegistryImpl.
        let config = self.config;
        let plugin_hooker_jsons = Self::load_plugin_hooker_jsons(&config.plugins)?;
        let mut hookers = HashMap::new();

        for hooker in build_hookers(&plugin_hooker_jsons)? {
            let hooker_id = hooker.id().clone();

            if hookers.insert(hooker_id.clone(), hooker).is_some() {
                return Err(BuildError::InvalidConfig {
                    message: format!("duplicate hooker id in registry: {}", hooker_id),
                });
            }
        }

        let registered_hooker_ids: HashSet<_> = hookers.keys().cloned().collect();

        for hooker_id in &config.enabled {
            if !registered_hooker_ids.contains(hooker_id) {
                return Err(BuildError::InvalidConfig {
                    message: format!("unknown enabled hooker id: {}", hooker_id),
                });
            }
        }

        for hooker_id in &config.disabled {
            if !registered_hooker_ids.contains(hooker_id) {
                return Err(BuildError::InvalidConfig {
                    message: format!("unknown disabled hooker id: {}", hooker_id),
                });
            }
        }

        for hooker_id in config.policies.keys() {
            if !registered_hooker_ids.contains(hooker_id) {
                return Err(BuildError::InvalidConfig {
                    message: format!("unknown policy hooker id: {}", hooker_id),
                });
            }
        }

        for hooker_id in &config.enabled {
            if config.disabled.contains(hooker_id) {
                return Err(BuildError::InvalidConfig {
                    message: format!(
                        "hooker id appears in both enabled and disabled: {}",
                        hooker_id
                    ),
                });
            }
        }

        let mut enabled_hookers = match config.default {
            HookerDefaultMode::All => registered_hooker_ids.clone(),
            HookerDefaultMode::None => HashSet::new(),
        };

        enabled_hookers.extend(config.enabled.iter().cloned());

        for hooker_id in &config.disabled {
            enabled_hookers.remove(hooker_id);
        }

        Ok(Box::new(HookerRegistryImpl::new(
            hookers,
            enabled_hookers,
            config.policies,
        )))
    }
}
