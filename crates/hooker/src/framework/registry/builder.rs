use std::collections::{HashMap, HashSet};

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
        let mut hookers = HashMap::new();

        for hooker in build_hookers(&[])? {
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
