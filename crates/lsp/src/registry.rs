use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_contracts::backend::OperationBackend;
use agent_contracts::lsp::LspProvider;

use crate::host::LocalLspEnv;
use crate::servers::ServerConfig;
use crate::service::LspService;

/// Caches one [`LspService`] per backend, keyed by `backend_id`.
///
/// When a tool runs with a specific operation backend it calls
/// [`LspServiceRegistry::get_or_create`] to obtain the matching service.
/// LSP server processes are therefore tied to the backend they serve —
/// a local backend shares its processes across all sessions that use it,
/// while a conch backend would have its own set (once supported).
pub struct LspServiceRegistry {
    extra_configs: Vec<ServerConfig>,
    services: Mutex<HashMap<String, Arc<dyn LspProvider>>>,
}

impl LspServiceRegistry {
    pub fn new(extra_configs: Vec<ServerConfig>) -> Self {
        Self {
            extra_configs,
            services: Mutex::new(HashMap::new()),
        }
    }

    /// Return the cached service for `backend`, or create and cache a new one.
    ///
    /// Returns `None` for backends that do not advertise LSP support.
    pub fn get_or_create(
        &self,
        backend: Arc<dyn OperationBackend>,
    ) -> Option<Arc<dyn LspProvider>> {
        if !backend.capabilities().supports_lsp {
            return None;
        }

        let backend_id = backend.backend_id().to_string();
        let mut services = self.services.lock().expect("lsp registry lock");

        if let Some(svc) = services.get(&backend_id) {
            return Some(Arc::clone(svc));
        }

        let env = Arc::new(LocalLspEnv::new(Arc::clone(&backend)));
        let svc =
            Arc::new(LspService::new(self.extra_configs.clone(), env)) as Arc<dyn LspProvider>;
        services.insert(backend_id, Arc::clone(&svc));
        Some(svc)
    }
}
