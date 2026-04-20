//! Sandbox isolation module for secure command execution.
//!
//! This module provides low-level OS isolation primitives for sandboxed
//! execution, including Landlock LSM, seccomp BPF, namespace isolation,
//! and mount isolation.
//!
//! # Architecture
//!
//! The sandbox module is designed to be called from the execute module's
//! pipeline. It provides:
//!
//! - **Landlock**: Filesystem access control via Linux Landlock LSM
//! - **Seccomp**: System call filtering via BPF
//! - **Namespaces**: Process isolation via Linux namespaces
//! - **Mount Isolation**: Fallback filesystem isolation via pivot_root
//!
//! # Platform Support
//!
//! All sandbox functionality is Linux-specific. On other platforms,
//! all functions return `UnsupportedPlatform` errors.
//!
//! # Capability Detection
//!
//! Use [`detect_capabilities()`] to check which sandbox features are available
//! on the current platform. This allows policies to fail closed when
//! enforcement cannot be guaranteed.
//!
//! # Example
//!
//! ```ignore
//! use cerberus_core::sandbox::SandboxSetup;
//! use cerberus_core::policy::Policy;
//!
//! let policy = Policy::strict();
//! let setup = SandboxSetup::new(&policy);
//! setup.setup()?;
//! ```

pub mod capability;
pub mod isolation;
pub mod namespace;
pub mod process;
pub mod setup;

#[cfg(feature = "ebpf")]
pub mod network;

pub use capability::{
    check_policy_runtime_requirements, check_strict_policy_enforceable, detect_capabilities,
    CapabilityLevel, EnforcementLevel, NamespaceCapability, SandboxCapabilities,
};
pub use isolation::{check_landlock_support, check_mount_isolation_support};
pub use namespace::clean_environment;
pub use process::{SandboxProcess, SpawnOptions};
pub use setup::SandboxSetup;

#[cfg(target_os = "linux")]
pub(crate) use namespace::{setup_parent_user_namespace, UserNamespaceSync};
