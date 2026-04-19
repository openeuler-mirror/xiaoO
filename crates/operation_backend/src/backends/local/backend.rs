use crate::backends::local::{
    exec::LocalExec, export::LocalExport, filesystem::LocalFileSystem, path::LocalPathResolver,
    search::LocalSearch,
};
use agent_contracts::backend::{
    capability::{
        OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
    },
    OperationBackend, OperationBackendCapabilities,
};

pub(crate) struct LocalBackendState {
    pub(crate) backend_id: String,
}

pub struct LocalOperationBackend {
    backend_id: String,
    capabilities: OperationBackendCapabilities,
    paths: LocalPathResolver,
    files: LocalFileSystem,
    search: LocalSearch,
    exec: LocalExec,
    export: LocalExport,
}

impl LocalOperationBackend {
    pub(crate) fn new(state: std::sync::Arc<LocalBackendState>) -> Self {
        Self {
            backend_id: state.backend_id.clone(),
            capabilities: OperationBackendCapabilities {
                supports_atomic_write: true,
                supports_grep: true,
                supports_export_file: true,
            },
            paths: LocalPathResolver::new(std::sync::Arc::clone(&state)),
            files: LocalFileSystem::new(std::sync::Arc::clone(&state)),
            search: LocalSearch::new(std::sync::Arc::clone(&state)),
            exec: LocalExec::new(std::sync::Arc::clone(&state)),
            export: LocalExport::new(state),
        }
    }
}

impl OperationBackend for LocalOperationBackend {
    fn backend_id(&self) -> &str {
        self.backend_id.as_str()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        self.capabilities
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        &self.paths as &dyn OperationPathResolver
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files as &dyn OperationFileSystem
    }

    fn search(&self) -> &dyn OperationSearch {
        &self.search as &dyn OperationSearch
    }

    fn exec(&self) -> &dyn OperationExec {
        &self.exec as &dyn OperationExec
    }

    fn export(&self) -> &dyn OperationExport {
        &self.export as &dyn OperationExport
    }
}
