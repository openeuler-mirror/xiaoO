use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_types::lsp::{
    LspCallHierarchyItem, LspDiagnostic, LspError, LspIncomingCall, LspLocation, LspOutgoingCall,
    LspSymbol,
};

use crate::host::LspEnv;
use crate::instance::LspServerInstance;
use crate::servers::{builtin_servers, find_root, read_file, ServerConfig};

type InstanceKey = (String, PathBuf); // (server_id, workspace_root)

/// TTL for cached `find_root` results. Bounds staleness: a deleted marker
/// or a new marker at a higher ancestor is picked up after this elapses.
const ROOT_CACHE_TTL: Duration = Duration::from_secs(300);

/// A cached `find_root` result with the time it was computed.
struct CachedRoot {
    root: PathBuf,
    cached_at: Instant,
}

impl CachedRoot {
    fn is_expired(&self) -> bool {
        Instant::now().duration_since(self.cached_at) >= ROOT_CACHE_TTL
    }
}

pub struct LspServerManager {
    configs: Vec<ServerConfig>,
    instances: HashMap<InstanceKey, LspServerInstance>,
    env: Arc<dyn LspEnv>,
    /// Cache of `find_root` results, keyed by `(file, server_id)`. All
    /// results (including the fallback to the file's parent) are cached;
    /// entries expire after `ROOT_CACHE_TTL`, so a marker added or removed
    /// later is picked up once the entry goes stale.
    root_cache: HashMap<PathBuf, HashMap<String, CachedRoot>>,
}

impl LspServerManager {
    pub fn new(extra_configs: Vec<ServerConfig>, env: Arc<dyn LspEnv>) -> Self {
        Self::new_with_disabled(extra_configs, Vec::new(), env)
    }

    pub fn new_with_disabled(
        extra_configs: Vec<ServerConfig>,
        disabled_servers: Vec<String>,
        env: Arc<dyn LspEnv>,
    ) -> Self {
        let configs = server_configs_with_disabled(extra_configs, &disabled_servers);
        Self {
            configs,
            instances: HashMap::new(),
            env,
            root_cache: HashMap::new(),
        }
    }

    pub fn new_custom(configs: Vec<ServerConfig>, env: Arc<dyn LspEnv>) -> Self {
        Self {
            configs,
            instances: HashMap::new(),
            env,
            root_cache: HashMap::new(),
        }
    }

    /// Resolve the workspace root for `file` against `root_markers`, using
    /// the in-process cache to avoid re-walking ancestors on every LSP
    /// action. All results are cached and expire after `ROOT_CACHE_TTL`;
    /// staleness is bounded, so markers added or removed during the TTL
    /// window are picked up after expiry.
    async fn cached_find_root(
        &mut self,
        file: &Path,
        server_id: &str,
        root_markers: &[&str],
    ) -> PathBuf {
        // Fast path: unexpired cache hit.
        if let Some(per_server) = self.root_cache.get(file) {
            if let Some(entry) = per_server.get(server_id) {
                if !entry.is_expired() {
                    return entry.root.clone();
                }
            }
        }
        let root = find_root(file, root_markers, self.env.as_ref()).await;
        // Always cache the result. The previous "only cache if root !=
        // file.parent()" heuristic skipped the case where the marker is in
        // the file's direct parent (e.g. main.rs next to Cargo.toml),
        // defeating the cache for that common layout. Rely on the TTL to
        // surface markers added or removed after the entry was stored.
        self.root_cache
            .entry(file.to_path_buf())
            .or_default()
            .insert(
                server_id.to_string(),
                CachedRoot {
                    root: root.clone(),
                    cached_at: Instant::now(),
                },
            );
        root
    }

    /// Ensure all matching instances exist and are started; open the file in each.
    async fn prepare_file(&mut self, file: &Path) -> Result<Vec<InstanceKey>, LspError> {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        // Collect matching configs into an owned Vec so we don't hold a
        // borrow of `self.configs` across the `cached_find_root` await.
        let matching: Vec<(&str, &[&str])> = self
            .configs
            .iter()
            .filter(|c| c.extensions.contains(&ext.as_str()))
            .map(|c| (c.id, c.root_markers))
            .collect();

        let mut keys: Vec<InstanceKey> = Vec::new();
        for (id, root_markers) in matching {
            let root = self.cached_find_root(file, id, root_markers).await;
            keys.push((id.to_string(), root));
        }

        if keys.is_empty() {
            return Err(LspError::NoServerForFile(
                file.to_string_lossy().to_string(),
            ));
        }

        // Read file content via backend.
        let text = read_file(file, self.env.as_ref()).await;

        for key in &keys {
            let (id, root) = key;
            if !self.instances.contains_key(key) {
                let config = self
                    .configs
                    .iter()
                    .find(|c| c.id == id)
                    .expect("config exists")
                    .clone();
                self.instances.insert(
                    key.clone(),
                    LspServerInstance::new(config, root.clone(), Arc::clone(&self.env)),
                );
            }
            if let Some(inst) = self.instances.get_mut(key) {
                let _ = inst.open_file(file, text.clone()).await;
            }
        }

        let mut ready = Vec::new();
        for key in &keys {
            if let Some(inst) = self.instances.get_mut(key) {
                match inst.ensure_started().await {
                    Ok(()) => ready.push(key.clone()),
                    Err(e) => tracing::warn!(server = %key.0, "failed to start: {e}"),
                }
            }
        }

        Ok(ready)
    }

    /// Open `file` in all matching LSP servers without waiting for any result.
    pub async fn touch_file(&mut self, file: &Path) {
        let _ = self.prepare_file(file).await;
    }

    /// Open `file` with explicitly provided content, bypassing the internal
    /// `read_file()` call in `prepare_file`.
    ///
    /// Used when file bytes are sourced from an operation backend rather than
    /// discovered through the env. For the local backend this is
    /// belt-and-suspenders; for future remote backends this is the primary sync path.
    pub async fn touch_file_with_content(&mut self, file: &Path, content: String) {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        // Collect matching configs into an owned Vec so we don't hold a
        // borrow of `self.configs` across the `cached_find_root` await.
        let matching: Vec<(&str, &[&str])> = self
            .configs
            .iter()
            .filter(|c| c.extensions.contains(&ext.as_str()))
            .map(|c| (c.id, c.root_markers))
            .collect();

        let mut keys: Vec<InstanceKey> = Vec::new();
        for (id, root_markers) in matching {
            let root = self.cached_find_root(file, id, root_markers).await;
            keys.push((id.to_string(), root));
        }

        for key in &keys {
            let (id, root) = key;
            if !self.instances.contains_key(key) {
                let config = self
                    .configs
                    .iter()
                    .find(|c| c.id == id)
                    .expect("config exists")
                    .clone();
                self.instances.insert(
                    key.clone(),
                    LspServerInstance::new(config, root.clone(), Arc::clone(&self.env)),
                );
            }
            if let Some(inst) = self.instances.get_mut(key) {
                let _ = inst.open_file(file, content.clone()).await;
            }
        }
    }

    pub async fn diagnostics(&mut self, file: &Path) -> Result<Vec<LspDiagnostic>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.diagnostics(file).await {
                    Ok(d) => result.extend(d),
                    Err(e) => tracing::warn!(server = %key.0, "diagnostics error: {e}"),
                }
            }
        }
        Ok(result)
    }

    pub async fn hover(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Option<String>, LspError> {
        let keys = self.prepare_file(file).await?;
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.hover(file, line, col).await {
                    Ok(Some(text)) => return Ok(Some(text)),
                    Ok(None) => {}
                    Err(e) => tracing::warn!(server = %key.0, "hover error: {e}"),
                }
            }
        }
        Ok(None)
    }

    pub async fn definition(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.definition(file, line, col).await {
                    Ok(locs) => result.extend(locs),
                    Err(e) => tracing::warn!(server = %key.0, "definition error: {e}"),
                }
            }
        }
        Ok(deduplicate_locations(result))
    }

    pub async fn references(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
        include_declaration: bool,
    ) -> Result<Vec<LspLocation>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.references(file, line, col, include_declaration).await {
                    Ok(locs) => result.extend(locs),
                    Err(e) => tracing::warn!(server = %key.0, "references error: {e}"),
                }
            }
        }
        Ok(deduplicate_locations(result))
    }

    pub async fn document_symbols(&mut self, file: &Path) -> Result<Vec<LspSymbol>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.document_symbols(file).await {
                    Ok(syms) => result.extend(syms),
                    Err(e) => tracing::warn!(server = %key.0, "document_symbols error: {e}"),
                }
            }
        }
        Ok(result)
    }

    pub async fn workspace_symbols(
        &mut self,
        file: &Path,
        query: &str,
    ) -> Result<Vec<LspSymbol>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.workspace_symbols(query).await {
                    Ok(syms) => result.extend(syms),
                    Err(e) => tracing::warn!(server = %key.0, "workspace_symbols error: {e}"),
                }
            }
        }
        Ok(result)
    }

    pub async fn implementation(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.implementation(file, line, col).await {
                    Ok(locs) => result.extend(locs),
                    Err(e) => tracing::warn!(server = %key.0, "implementation error: {e}"),
                }
            }
        }
        Ok(deduplicate_locations(result))
    }

    pub async fn prepare_call_hierarchy(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspCallHierarchyItem>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.prepare_call_hierarchy(file, line, col).await {
                    Ok(items) => result.extend(items),
                    Err(e) => tracing::warn!(server = %key.0, "prepare_call_hierarchy error: {e}"),
                }
            }
        }
        Ok(result)
    }

    pub async fn incoming_calls(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspIncomingCall>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.incoming_calls(file, line, col).await {
                    Ok(calls) => result.extend(calls),
                    Err(e) => tracing::warn!(server = %key.0, "incoming_calls error: {e}"),
                }
            }
        }
        Ok(result)
    }

    pub async fn outgoing_calls(
        &mut self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspOutgoingCall>, LspError> {
        let keys = self.prepare_file(file).await?;
        let mut result = Vec::new();
        for key in keys {
            if let Some(inst) = self.instances.get(&key) {
                match inst.outgoing_calls(file, line, col).await {
                    Ok(calls) => result.extend(calls),
                    Err(e) => tracing::warn!(server = %key.0, "outgoing_calls error: {e}"),
                }
            }
        }
        Ok(result)
    }
}

fn server_configs_with_disabled(
    extra_configs: Vec<ServerConfig>,
    disabled_servers: &[String],
) -> Vec<ServerConfig> {
    let disabled_servers: HashSet<&str> = disabled_servers.iter().map(String::as_str).collect();
    let mut configs = extra_configs;
    configs.extend(builtin_servers());
    configs.retain(|config| !disabled_servers.contains(config.id));
    configs
}

fn deduplicate_locations(mut locs: Vec<LspLocation>) -> Vec<LspLocation> {
    locs.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    locs.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.col == b.col);
    locs
}

#[cfg(test)]
mod tests {
    use super::server_configs_with_disabled;
    use crate::servers::{AutoInstall, ServerConfig};

    #[test]
    fn server_configs_filter_disabled_builtin_and_extra_servers() {
        let extra = ServerConfig {
            id: "custom-python",
            extensions: &["py"],
            command: "custom-python-lsp",
            args: &[],
            root_markers: &["pyproject.toml"],
            language_id: "python",
            initialization_options: None,
            auto_install: AutoInstall::None,
        };

        let configs = server_configs_with_disabled(
            vec![extra],
            &["pyright".to_string(), "custom-python".to_string()],
        );
        let ids = configs.iter().map(|config| config.id).collect::<Vec<_>>();

        assert!(!ids.contains(&"pyright"));
        assert!(!ids.contains(&"custom-python"));
        assert!(ids.contains(&"rust-analyzer"));
    }
}
