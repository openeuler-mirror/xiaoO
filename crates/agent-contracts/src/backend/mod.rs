pub mod capability;
pub mod config;

mod builder;
mod contract;
mod error;
mod types;

pub use builder::{OperationBackendBuildError, OperationBackendBuilder};
pub use config::{OperationBackendBuildInput, OperationBackendConfig};
pub use contract::{OperationBackend, OperationBackendCapabilities, OperationBackendKind};
pub use error::OperationError;
pub use types::{
    BackendPath, ExportedFileHandle, ExportedFileMeta, ExportedFileReader, PathKind, PathStat,
    SharedExportedFileHandle,
};
