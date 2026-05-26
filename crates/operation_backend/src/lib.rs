mod backends;
mod builder;

pub use backends::local::local_backend_for_workspace;
pub use builder::OperationBackendBuilderImpl;

/// Build a minimal local backend for the LSP subsystem.
///
/// Uses the process home directory as workspace root. Suitable for
/// daemon-level singletons that need file / exec operations independently
/// of any per-session backend.
pub use backends::local::lsp_backend::local_lsp_backend;
