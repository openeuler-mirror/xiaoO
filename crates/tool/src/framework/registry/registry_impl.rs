use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolRegistry, ToolSpecView};
use agent_types::common::ids::{AgentId, ToolId};
use agent_types::tool::ToolVisibilityConfig;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use super::ToolFilterImpl;

/// Upper bound on cached per-agent filters. Each entry owns an
/// `Arc<ToolFilterImpl>` (two HashMaps + a Vec); unbounded growth in a
/// long-lived process with many short-lived agents would leak memory.
/// When exceeded we evict an arbitrary entry (weak LRU; HashMap iteration
/// order) — sufficient for the common case where a few long-lived agents
/// dominate lookups.
const FILTER_CACHE_MAX_ENTRIES: usize = 256;

pub struct ToolRegistryImpl {
    executors: HashMap<ToolId, Arc<dyn ToolExecutor>>,
    specs: HashMap<ToolId, Arc<dyn ToolSpecView>>,
    visibility_config: ToolVisibilityConfig,
    /// Per-agent cache for `filter_for`. The visibility config is immutable
    /// for the registry's lifetime, so a given `AgentId`'s filter never
    /// changes — caching avoids rebuilding the HashSet + two HashMaps +
    /// sort on every call. Bounded by `FILTER_CACHE_MAX_ENTRIES`.
    filter_cache: RwLock<HashMap<AgentId, Arc<ToolFilterImpl>>>,
}

impl ToolRegistryImpl {
    pub fn new(
        executors: HashMap<ToolId, Arc<dyn ToolExecutor>>,
        specs: HashMap<ToolId, Arc<dyn ToolSpecView>>,
        visibility_config: ToolVisibilityConfig,
    ) -> Self {
        Self {
            executors,
            specs,
            visibility_config,
            filter_cache: RwLock::new(HashMap::new()),
        }
    }

    fn resolve_visible_specs_for_agent(&self, agent_id: &AgentId) -> Vec<Arc<dyn ToolSpecView>> {
        let Some(allowed_tool_names) = self.visibility_config.per_agent_allowed_tools.get(agent_id)
        else {
            return Vec::new();
        };

        let allowed_tool_names: HashSet<_> = allowed_tool_names
            .iter()
            .map(|tool_name| tool_name.0.as_str())
            .collect();

        let mut visible_specs: Vec<_> = self
            .specs
            .values()
            .filter(|spec| allowed_tool_names.contains(spec.name().0.as_str()))
            .map(Arc::clone)
            .collect();
        visible_specs.sort_by(|left, right| left.name().0.cmp(&right.name().0));
        visible_specs
    }

    fn resolve_visible_executors_by_name(
        &self,
        visible_specs: &[Arc<dyn ToolSpecView>],
    ) -> HashMap<String, Arc<dyn ToolExecutor>> {
        visible_specs
            .iter()
            .filter_map(|spec| {
                self.executors
                    .get(spec.id())
                    .map(|executor| (spec.name().0.clone(), Arc::clone(executor)))
            })
            .collect()
    }
}

/// Thin wrapper returning a cached `Arc<ToolFilterImpl>` as a
/// `Box<dyn ToolFilter>` without re-allocating the underlying maps.
/// Cloning the Arc is a single atomic increment.
struct CachedToolFilter(Arc<ToolFilterImpl>);

impl ToolFilter for CachedToolFilter {
    fn visible_tools(&self) -> Vec<&dyn ToolSpecView> {
        self.0.visible_tools()
    }

    fn allows_tool_name(&self, tool_name: &str) -> bool {
        self.0.allows_tool_name(tool_name)
    }

    fn get_spec_for_name(&self, tool_name: &str) -> Option<Arc<dyn ToolSpecView>> {
        self.0.get_spec_for_name(tool_name)
    }

    fn get_executor_for_name(&self, tool_name: &str) -> Option<Arc<dyn ToolExecutor>> {
        self.0.get_executor_for_name(tool_name)
    }
}

impl ToolRegistry for ToolRegistryImpl {
    fn get_executor(&self, id: &ToolId) -> Option<Arc<dyn ToolExecutor>> {
        self.executors.get(id).map(Arc::clone)
    }

    fn get_spec(&self, id: &ToolId) -> Option<&dyn ToolSpecView> {
        self.specs.get(id).map(|spec| spec.as_ref())
    }

    fn list_specs(&self) -> Vec<&dyn ToolSpecView> {
        let mut specs: Vec<_> = self
            .specs
            .values()
            .map(|spec| spec.as_ref() as &dyn ToolSpecView)
            .collect();
        specs.sort_by(|left, right| left.name().0.cmp(&right.name().0));
        specs
    }

    fn spec_count(&self) -> usize {
        self.specs.len()
    }

    fn filter_for(&self, agent_id: &AgentId) -> Box<dyn ToolFilter> {
        // Fast path: read lock, look up, return Arc clone.
        if let Ok(cache) = self.filter_cache.read() {
            if let Some(cached) = cache.get(agent_id) {
                return Box::new(CachedToolFilter(Arc::clone(cached)));
            }
        }
        // Slow path: build a fresh filter, then write-lock and insert.
        // `entry().or_insert_with` is idempotent — if another thread
        // populated the entry while we were building, we discard our
        // locally-built Arc (functionally identical to the cached one).
        // The wasted build is the thundering-herd cost of the first miss
        // for a given agent; subsequent calls hit the fast path.
        let built = {
            let visible_specs = self.resolve_visible_specs_for_agent(agent_id);
            let executors_by_name = self.resolve_visible_executors_by_name(&visible_specs);
            Arc::new(ToolFilterImpl::new(visible_specs, executors_by_name))
        };
        if let Ok(mut cache) = self.filter_cache.write() {
            if cache.len() >= FILTER_CACHE_MAX_ENTRIES && !cache.contains_key(agent_id) {
                // Evict one arbitrary entry to make room (weak LRU; the
                // cache is a performance hint, not a correctness requirement).
                if let Some(evict_key) = cache.keys().next().cloned() {
                    cache.remove(&evict_key);
                }
            }
            cache
                .entry(agent_id.clone())
                .or_insert_with(|| Arc::clone(&built));
        }
        Box::new(CachedToolFilter(built))
    }
}
