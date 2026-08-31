//! Operation-plane contracts for caller-owned tool sandboxes.
//!
//! This module intentionally exposes operations only.  Backend lifecycle,
//! pooling, quotas, checkpoints, and provider/config resolution are outside
//! the runtime SDK.

#[doc(inline)]
pub use agent_contracts::backend::{
    BackendPath, ExecutionState, ExportedFileHandle, ExportedFileMeta, ExportedFileReader,
    OperationBackend, OperationBackendCapabilities, OperationError, OperationPermissionControl,
    PathKind, PathStat, SandboxPermissionCapability, SandboxPermissionGrantId,
    SandboxPermissionGrantRequest, SandboxPermissionScope, SandboxPolicyDenial,
    SharedExportedFileHandle,
};

#[doc(inline)]
pub use agent_contracts::backend::capability::{
    OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
};

#[doc(inline)]
pub use agent_contracts::backend::capability::exec::{ExecRequest, ExecResult};
#[doc(inline)]
pub use agent_contracts::backend::capability::export::ExportFileRequest;
#[doc(inline)]
pub use agent_contracts::backend::capability::filesystem::{
    ReadBytesRequest, TempPathKind, TempPathRequest, WriteBytesOutcome, WriteBytesRequest,
    WriteMode,
};
#[doc(inline)]
pub use agent_contracts::backend::capability::path::{ResolveBase, ResolvePathRequest};
#[doc(inline)]
pub use agent_contracts::backend::capability::search::{
    GlobRequest, GrepMode, GrepRequest, GrepResult,
};
