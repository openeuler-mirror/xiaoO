use std::collections::{HashMap, HashSet};

use agent_contracts::hooker::{Hooker, HookerRegistry};
use agent_types::common::HookerId;
use agent_types::hooker::HookPointId;

pub struct HookerRegistryImpl {
    // Stores all registered hookers (tool and LLM) by HookerId.
    hookers: HashMap<HookerId, Box<dyn Hooker>>,
    // Stores which hookers are enabled by the registry config.
    enabled_hookers: HashSet<HookerId>,
    // Stores per-hooker policy payloads from the registry config.
    policies: HashMap<HookerId, serde_json::Value>,
}

impl HookerRegistryImpl {
    pub fn new(
        hookers: HashMap<HookerId, Box<dyn Hooker>>,
        enabled_hookers: HashSet<HookerId>,
        policies: HashMap<HookerId, serde_json::Value>,
    ) -> Self {
        Self {
            hookers,
            enabled_hookers,
            policies,
        }
    }
}

impl HookerRegistry for HookerRegistryImpl {
    fn get(&self, id: &HookerId) -> Option<&dyn Hooker> {
        self.hookers.get(id).map(Box::as_ref)
    }

    fn list(&self) -> Vec<&dyn Hooker> {
        self.hookers.values().map(Box::as_ref).collect()
    }

    fn list_for_hook_point(&self, hook_point: &HookPointId) -> Vec<&dyn Hooker> {
        self.hookers
            .values()
            .filter(|hooker| hooker.hook_point() == hook_point)
            .map(Box::as_ref)
            .collect()
    }

    fn is_enabled(&self, id: &HookerId) -> bool {
        self.enabled_hookers.contains(id)
    }

    fn policy_for(&self, id: &HookerId) -> Option<&serde_json::Value> {
        self.policies.get(id)
    }
}
