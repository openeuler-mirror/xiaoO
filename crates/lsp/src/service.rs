use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_contracts::lsp::LspProvider;
use agent_types::lsp::{
    LspCallHierarchyItem, LspDiagnostic, LspError, LspIncomingCall, LspLocation, LspOutgoingCall,
    LspSymbol,
};

use crate::host::LspEnv;
use crate::manager::LspServerManager;
use crate::servers::ServerConfig;

pub struct LspService {
    manager: Arc<Mutex<LspServerManager>>,
}

impl LspService {
    pub fn new(extra_configs: Vec<ServerConfig>, env: Arc<dyn LspEnv>) -> Self {
        Self {
            manager: Arc::new(Mutex::new(LspServerManager::new(extra_configs, env))),
        }
    }

    /// Use exactly the provided configs without adding built-in language servers.
    pub fn new_custom(configs: Vec<ServerConfig>, env: Arc<dyn LspEnv>) -> Self {
        Self {
            manager: Arc::new(Mutex::new(LspServerManager::new_custom(configs, env))),
        }
    }
}

#[async_trait]
impl LspProvider for LspService {
    async fn diagnostics(&self, file: &Path) -> Result<Vec<LspDiagnostic>, LspError> {
        self.manager.lock().await.diagnostics(file).await
    }

    async fn hover(&self, file: &Path, line: u32, col: u32) -> Result<Option<String>, LspError> {
        self.manager.lock().await.hover(file, line, col).await
    }

    async fn definition(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        self.manager.lock().await.definition(file, line, col).await
    }

    async fn references(
        &self,
        file: &Path,
        line: u32,
        col: u32,
        include_declaration: bool,
    ) -> Result<Vec<LspLocation>, LspError> {
        self.manager
            .lock()
            .await
            .references(file, line, col, include_declaration)
            .await
    }

    async fn symbols(&self, file: &Path, query: Option<&str>) -> Result<Vec<LspSymbol>, LspError> {
        let mut mgr = self.manager.lock().await;
        if let Some(q) = query {
            mgr.workspace_symbols(file, q).await
        } else {
            mgr.document_symbols(file).await
        }
    }

    async fn implementation(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError> {
        self.manager
            .lock()
            .await
            .implementation(file, line, col)
            .await
    }

    async fn prepare_call_hierarchy(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspCallHierarchyItem>, LspError> {
        self.manager
            .lock()
            .await
            .prepare_call_hierarchy(file, line, col)
            .await
    }

    async fn incoming_calls(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspIncomingCall>, LspError> {
        self.manager
            .lock()
            .await
            .incoming_calls(file, line, col)
            .await
    }

    async fn outgoing_calls(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspOutgoingCall>, LspError> {
        self.manager
            .lock()
            .await
            .outgoing_calls(file, line, col)
            .await
    }

    async fn touch_file(&self, file: &Path) {
        self.manager.lock().await.touch_file(file).await;
    }

    async fn open_file(&self, file: &Path, content: String) {
        self.manager
            .lock()
            .await
            .touch_file_with_content(file, content)
            .await;
    }
}
