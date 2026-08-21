//! Process execution with namespace isolation.

mod r#impl;

pub use r#impl::{SandboxProcess, SpawnOptions};

#[cfg(target_os = "linux")]
pub(crate) use r#impl::apply_resource_limits;
