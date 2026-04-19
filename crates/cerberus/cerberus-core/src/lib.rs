//! Cerberus core library
//!
//! Secure command execution with policy enforcement.
//!
//! # Example
//!
//! ```rust,no_run
//! use cerberus_core::request::ExecRequest;
//! use cerberus_core::policy::Policy;
//! use cerberus_core::execute::execute;
//!
//! let request = ExecRequest::new("ls").arg("-la");
//! let policy = Policy::minimal();
//! let result = execute(request, &policy)?;
//!
//! println!("Exit code: {}", result.exit_code);
//! println!("Output: {}", result.stdout_utf8());
//! # Ok::<(), cerberus_core::CerberusError>(())
//! ```

pub mod audit;
pub mod error;
pub mod execute;
pub mod filters;
pub mod network;
pub mod policy;
pub mod request;
pub mod result;
pub mod sandbox;

#[cfg(feature = "ebpf")]
pub mod ebpf;

pub use audit::{
    AuditEvent,
    AuditSink,
    // eBPF audit types
    BpfRawEvent,
    CompositeObserver,
    CoreExecEvent,
    EbpfAuditEvent,
    EventType,
    ExecObserver,
    ExecutionContext,
    ExecutionContextBuilder,
    FileAccessEvent,
    FileAccessResult,
    FileOperation,
    FileSink,
    ForkEvent,
    LoggingObserver,
    MemorySink,
    MetricsObserver,
    MultiSink,
    NetworkAccessEvent,
    NetworkAccessResult,
    NetworkDirection,
    NetworkProtocol,
    NoOpObserver,
    NoOpSink,
    RequestId,
    SyscallEvent,
    SyscallResult,
    TimeoutEvent,
};
pub use error::{
    AuditError, CerberusError, ExecutionError, FilterError, PolicyError, RequestError,
    SandboxSetupError,
};
pub use execute::{
    execute, execute_shell, execute_shell_capture, execute_shell_with_observer,
    execute_with_observer,
};
pub use filters::{
    ArgFilter, ArgFilterConfig, EnvFilter, EnvFilterConfig, ExecutionControl,
    ExecutionControlBuilder, FilteredRequest, OutputFilter, OutputFilterConfig, OutputFilterResult,
    RedactPattern, ViolationAction, ViolationResult,
};
pub use policy::{
    EnvironmentConfig, FsPermission, FsRule, NamespaceConfig, NetworkAction, NetworkPolicy,
    NetworkPolicyMode, NetworkRule, PathGroups, Policy, PolicyBuilder, PortRange, ResourceLimits,
};
pub use request::{ExecRequest, OutputPolicy, StdinPolicy};
pub use result::{ExecMetadata, ExecResult};
pub use sandbox::{SandboxProcess, SandboxSetup, SpawnOptions};

#[cfg(feature = "ebpf")]
pub use ebpf::{EbpfAuditBackend, EbpfAuditBackendBuilder, EbpfLoadError, EbpfLoader};
