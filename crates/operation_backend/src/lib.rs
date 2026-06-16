mod backends;
mod builder;
pub mod process_group;

pub use backends::local::{
    local_backend, local_backend_provider, local_backend_with_isolation, LocalBackendProvider,
};
pub use builder::OperationBackendBuilderImpl;

/// Build a minimal local backend for the LSP subsystem.
///
/// Uses the process home directory as workspace root. Suitable for
/// daemon-level singletons that need file / exec operations independently
/// of any per-session backend.
pub use backends::local::lsp_backend::local_lsp_backend;
