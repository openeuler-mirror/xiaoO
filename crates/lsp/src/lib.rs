mod client;
mod instance;
mod manager;

pub mod host;
pub mod registry;
pub mod servers;
pub mod service;
pub mod types;

pub use agent_contracts::lsp::LspProvider;
pub use agent_types::lsp::LspError;
pub use host::{LocalLspEnv, LspEnv};
pub use registry::LspServiceRegistry;
pub use servers::{AutoInstall, ServerConfig};
pub use service::LspService;
pub use types::{LspDiagnostic, LspLocation, LspSymbol};
