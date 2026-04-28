use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_types::lsp::{
    LspCallHierarchyItem, LspDiagnostic, LspError, LspIncomingCall, LspLocation, LspOutgoingCall,
    LspSymbol,
};

use crate::host::LspEnv;
use crate::instance::LspServerInstance;
use crate::servers::{builtin_servers, find_root, read_file, ServerConfig};

type InstanceKey = (String, PathBuf); // (server_id, workspace_root)

pub struct LspServerManager {
    configs: Vec<ServerConfig>,
    instances: HashMap<InstanceKey, LspServerInstance>,
    env: Arc<dyn LspEnv>,
}

impl LspServerManager {
    pub fn new(extra_configs: Vec<ServerConfig>, env: Arc<dyn LspEnv>) -> Self {
        let mut configs = extra_configs;
        configs.extend(builtin_servers());
        Self {
            configs,
            instances: HashMap::new(),
            env,
        }
    }

    pub fn new_custom(configs: Vec<ServerConfig>, env: Arc<dyn LspEnv>) -> Self {
        Self {
            configs,
            instances: HashMap::new(),
            env,
        }
    }

    /// Ensure all matching instances exist and are started; open the file in each.
    async fn prepare_file(&mut self, file: &Path) -> Result<Vec<InstanceKey>, LspError> {
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        let mut keys: Vec<InstanceKey> = Vec::new();
        for c in self.configs.iter().filter(|c| c.extensions.contains(&ext.as_str())) {
            let root = find_root(file, c.root_markers, self.env.as_ref()).await;
            keys.push((c.id.to_string(), root));
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

        let mut keys: Vec<InstanceKey> = Vec::new();
        for c in self.configs.iter().filter(|c| c.extensions.contains(&ext.as_str())) {
            let root = find_root(file, c.root_markers, self.env.as_ref()).await;
            keys.push((c.id.to_string(), root));
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

fn deduplicate_locations(mut locs: Vec<LspLocation>) -> Vec<LspLocation> {
    locs.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    locs.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.col == b.col);
    locs
}
