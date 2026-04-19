//! Namespace management for sandbox isolation.

mod r#impl;

pub use r#impl::clean_environment;

#[cfg(target_os = "linux")]
pub(crate) use r#impl::{
    apply_namespaces, remount_procfs, runtime_namespace_capabilities, setup_parent_user_namespace,
    UserNamespaceSync,
};
