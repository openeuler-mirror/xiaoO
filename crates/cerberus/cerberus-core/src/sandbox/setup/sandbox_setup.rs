//! Sandbox setup configuration and execution.

use crate::error::SandboxSetupError;
use crate::policy::{FsRule, NamespaceConfig, NetworkPolicy, Policy, ResourceLimits};

#[cfg(target_os = "linux")]
use crate::sandbox::capability::{
    check_namespace_capabilities, ensure_network_policy_backend_available,
    missing_requested_namespaces,
};
#[cfg(target_os = "linux")]
use crate::sandbox::namespace::{self, UserNamespaceSync};
#[cfg(target_os = "linux")]
use crate::sandbox::process;

/// Configuration for sandbox setup.
///
/// This struct holds all the configuration needed to set up
/// sandbox isolation before executing a process.
#[derive(Debug, Clone)]
pub struct SandboxSetup {
    /// Filesystem rules for Landlock/mount isolation.
    pub fs_rules: Vec<FsRule>,
    /// Namespace configuration.
    pub namespaces: NamespaceConfig,
    /// Resource limits applied before exec.
    pub resources: ResourceLimits,
    pub network_policy: Option<NetworkPolicy>,
    /// Whether Landlock is optional (use mount fallback on failure).
    pub landlock_optional: bool,
    /// Whether to use mount isolation as fallback.
    pub mount_isolation_fallback: bool,
    /// Environment variable whitelist.
    pub env_whitelist: Vec<String>,
}

impl SandboxSetup {
    /// Create a new sandbox setup from a policy.
    pub fn new(policy: &Policy) -> Self {
        Self {
            fs_rules: policy.fs_rules(),
            namespaces: policy.namespaces.clone(),
            resources: policy.resources.clone(),
            network_policy: policy.network_policy.clone(),
            landlock_optional: policy.landlock_optional,
            mount_isolation_fallback: policy.mount_isolation_fallback,
            env_whitelist: policy.environment.whitelist.clone(),
        }
    }

    /// Perform sandbox setup in the current process.
    ///
    /// This should be called in a forked child process before exec.
    /// It validates namespace requests, applies Landlock/seccomp rules,
    /// and prepares the environment for isolated execution.
    ///
    /// # Errors
    ///
    /// Returns `SandboxSetupError` if any isolation setup fails.
    #[cfg(target_os = "linux")]
    pub fn setup(&self) -> Result<(), SandboxSetupError> {
        self.setup_with_namespace_sync(None)
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn setup_with_namespace_sync(
        &self,
        user_sync: Option<UserNamespaceSync>,
    ) -> Result<(), SandboxSetupError> {
        namespace::clean_environment(&self.env_whitelist);
        self.ensure_network_policy_runtime_requirements()?;
        self.ensure_supported_namespaces()?;
        let applied_namespaces = namespace::apply_namespaces(&self.namespaces, user_sync)?;
        close_user_namespace_sync(user_sync);

        if applied_namespaces.pid_fork_required {
            enter_pid_namespace_child()?;
            if self.namespaces.mount {
                namespace::remount_procfs()?;
            }
        }

        self.setup_filesystem_isolation()?;
        process::apply_resource_limits(&self.resources)?;
        crate::sandbox::isolation::apply_seccomp_filter()
            .map_err(|e| SandboxSetupError::SeccompSetupFailed(e.to_string()))?;

        Ok(())
    }

    /// Perform sandbox setup (non-Linux stub).
    #[cfg(not(target_os = "linux"))]
    pub fn setup(&self) -> Result<(), SandboxSetupError> {
        Err(SandboxSetupError::UnsupportedPlatform)
    }

    /// Set up filesystem isolation via Landlock or mount isolation fallback.
    #[cfg(target_os = "linux")]
    fn setup_filesystem_isolation(&self) -> Result<(), SandboxSetupError> {
        match crate::sandbox::isolation::apply_landlock_rules(&self.fs_rules) {
            Ok(()) => Ok(()),
            Err(e) if self.landlock_optional => {
                if self.mount_isolation_fallback {
                    crate::sandbox::isolation::apply_mount_isolation(&self.fs_rules)
                        .map_err(|e| SandboxSetupError::MountIsolationFailed(e.to_string()))
                } else {
                    eprintln!(
                        "Warning: Landlock unavailable ({}), continuing without filesystem isolation",
                        e
                    );
                    Ok(())
                }
            }
            Err(e) => Err(SandboxSetupError::LandlockSetupFailed(e.to_string())),
        }
    }

    #[cfg(target_os = "linux")]
    fn ensure_supported_namespaces(&self) -> Result<(), SandboxSetupError> {
        let (missing_critical_namespaces, _) =
            missing_requested_namespaces(&self.namespaces, &check_namespace_capabilities());

        if missing_critical_namespaces.is_empty() {
            return Ok(());
        }

        Err(SandboxSetupError::CapabilityError {
            feature: "namespaces".to_string(),
            reason: format!(
                "requested Linux namespace isolation is not enforced by the active runtime: {}",
                missing_critical_namespaces.join(", ")
            ),
        })
    }

    #[cfg(target_os = "linux")]
    fn ensure_network_policy_runtime_requirements(&self) -> Result<(), SandboxSetupError> {
        if let Some(network_policy) = &self.network_policy {
            if network_policy.is_enabled() {
                ensure_network_policy_backend_available(network_policy)?;
            }
        }

        Ok(())
    }

    /// Check if sandbox functionality is available on this platform.
    pub fn is_available() -> bool {
        #[cfg(target_os = "linux")]
        {
            crate::sandbox::isolation::check_landlock_support().is_ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }
}

#[cfg(target_os = "linux")]
fn close_user_namespace_sync(user_sync: Option<UserNamespaceSync>) {
    if let Some(user_sync) = user_sync {
        close_namespace_sync_fd(user_sync.notify_parent_fd);
        close_namespace_sync_fd(user_sync.wait_parent_fd);
    }
}

#[cfg(target_os = "linux")]
fn close_namespace_sync_fd(fd: libc::c_int) {
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

#[cfg(target_os = "linux")]
fn enter_pid_namespace_child() -> Result<(), SandboxSetupError> {
    let grandchild = unsafe { libc::fork() };
    if grandchild < 0 {
        return Err(SandboxSetupError::NamespaceSetupFailed(format!(
            "fork after pid namespace setup failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    if grandchild > 0 {
        mirror_pid_namespace_child(grandchild);
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn mirror_pid_namespace_child(grandchild: libc::pid_t) -> ! {
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(grandchild, &mut status, 0) };
        if result == grandchild {
            if libc::WIFEXITED(status) {
                unsafe {
                    libc::_exit(libc::WEXITSTATUS(status));
                }
            }

            if libc::WIFSIGNALED(status) {
                let signal = libc::WTERMSIG(status);
                unsafe {
                    libc::signal(signal, libc::SIG_DFL);
                    libc::raise(signal);
                    libc::_exit(128 + signal);
                }
            }

            unsafe {
                libc::_exit(1);
            }
        }

        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }

            unsafe {
                libc::_exit(1);
            }
        }
    }
}

impl Default for SandboxSetup {
    fn default() -> Self {
        Self::new(&Policy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sandbox_setup_from_policy() {
        let policy = Policy::strict();
        let setup = SandboxSetup::new(&policy);

        assert!(!setup.fs_rules.is_empty());
        assert!(setup.namespaces.mount);
        assert!(!setup.namespaces.network);
        assert!(setup.network_policy.is_none());
        assert_eq!(setup.resources.timeout_secs, policy.resources.timeout_secs);
    }

    #[test]
    fn test_sandbox_setup_default() {
        let setup = SandboxSetup::default();
        assert!(!setup.fs_rules.is_empty());
    }

    #[test]
    fn test_is_available() {
        #[cfg(target_os = "linux")]
        {
            let _ = SandboxSetup::is_available();
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert!(!SandboxSetup::is_available());
        }
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_setup_unsupported_platform() {
        let setup = SandboxSetup::default();
        let result = setup.setup();
        assert!(matches!(
            result,
            Err(SandboxSetupError::UnsupportedPlatform)
        ));
    }

    #[test]
    fn test_sandbox_setup_carries_network_policy_forward() {
        let mut policy = Policy::with_network();
        policy.network_policy = Some(NetworkPolicy::default());

        let setup = SandboxSetup::new(&policy);

        assert!(setup.network_policy.is_some());
        assert!(setup.namespaces.network);
    }
}
