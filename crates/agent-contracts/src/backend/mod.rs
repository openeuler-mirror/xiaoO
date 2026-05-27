pub mod capability;
pub mod config;
pub mod permission;

mod builder;
mod contract;
mod error;
mod types;

pub use builder::{OperationBackendBuildError, OperationBackendBuilder};
pub use config::{OperationBackendBuildInput, OperationBackendConfig};
pub use contract::{OperationBackend, OperationBackendCapabilities};
pub use error::OperationError;
pub use permission::{
    OperationPermissionControl, SandboxPermissionCapability, SandboxPermissionGrantId,
    SandboxPermissionGrantRequest, SandboxPermissionScope, SandboxPolicyDenial,
};
pub use types::{
    BackendPath, ExportedFileHandle, ExportedFileMeta, ExportedFileReader, PathKind, PathStat,
    SharedExportedFileHandle,
};
