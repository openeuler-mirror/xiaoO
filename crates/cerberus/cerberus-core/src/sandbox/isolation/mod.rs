//! Isolation mechanisms for sandbox execution.

mod landlock;
mod mount;
mod seccomp;

pub use landlock::check_landlock_support;
pub use mount::check_mount_isolation_support;

#[cfg(target_os = "linux")]
pub(crate) use landlock::apply_landlock_rules;
#[cfg(target_os = "linux")]
pub(crate) use mount::apply_mount_isolation;
#[cfg(target_os = "linux")]
pub(crate) use seccomp::apply_seccomp_filter;
