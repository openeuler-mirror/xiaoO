use std::fmt;

/// Capability requested by a sandboxed backend operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPermissionCapability {
    Read,
    Write,
    ExecCwd,
    ExecRuntime,
}

impl fmt::Display for SandboxPermissionCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => f.write_str("read"),
            Self::Write => f.write_str("write"),
            Self::ExecCwd => f.write_str("exec_cwd"),
            Self::ExecRuntime => f.write_str("exec_runtime"),
        }
    }
}

/// Lifetime of a user-approved sandbox permission grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxPermissionScope {
    Once,
    Session,
}

/// Structured reason emitted when a sandbox policy blocks an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPolicyDenial {
    pub backend_id: String,
    pub isolation: String,
    pub operation: String,
    pub capability: SandboxPermissionCapability,
    pub path: String,
}

impl fmt::Display for SandboxPolicyDenial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} blocked {} {} for {}",
            self.isolation, self.capability, self.path, self.operation
        )
    }
}

#[derive(Debug, Clone)]
pub struct SandboxPermissionGrantRequest {
    pub denial: SandboxPolicyDenial,
    pub scope: SandboxPermissionScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SandboxPermissionGrantId(pub u64);

pub trait OperationPermissionControl: Send + Sync {
    fn grant(
        &self,
        request: SandboxPermissionGrantRequest,
    ) -> Result<SandboxPermissionGrantId, super::OperationError>;

    fn revoke(&self, id: SandboxPermissionGrantId) -> Result<(), super::OperationError>;
}
