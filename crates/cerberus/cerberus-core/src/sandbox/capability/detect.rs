//! Sandbox capability detection functions.

use super::level::{CapabilityLevel, EnforcementLevel};
use super::namespace::NamespaceCapability;
use super::types::SandboxCapabilities;
use crate::error::SandboxSetupError;
use crate::policy::{NamespaceConfig, NetworkPolicy, NetworkPolicyMode, Policy};

#[cfg(target_os = "linux")]
use crate::sandbox::namespace;

/// Detect sandbox capabilities for the current platform.
///
/// This function probes the system to determine which sandbox isolation
/// features are available. Use this to:
/// - Display capability status in profile output
/// - Decide whether to fail closed or allow fallback execution
/// - Understand why a policy may not be fully enforced
///
/// # Returns
///
/// A `SandboxCapabilities` struct describing the availability of each
/// isolation feature.
pub fn detect_capabilities() -> SandboxCapabilities {
    #[cfg(target_os = "linux")]
    {
        detect_capabilities_linux()
    }

    #[cfg(not(target_os = "linux"))]
    {
        SandboxCapabilities {
            landlock: CapabilityLevel::Unavailable,
            seccomp: CapabilityLevel::Unavailable,
            namespaces: NamespaceCapability {
                mount: false,
                pid: false,
                network: false,
                user: false,
            },
            mount_isolation: CapabilityLevel::Unavailable,
            enforcement_level: EnforcementLevel::Unsupported,
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_capabilities_linux() -> SandboxCapabilities {
    let landlock = if crate::sandbox::isolation::check_landlock_support().is_ok() {
        CapabilityLevel::Full
    } else {
        CapabilityLevel::Unavailable
    };

    let seccomp = check_seccomp_available();
    let namespaces = check_namespace_capabilities();
    let mount_isolation = if crate::sandbox::isolation::check_mount_isolation_support().is_ok() {
        CapabilityLevel::Full
    } else {
        CapabilityLevel::Unavailable
    };

    let enforcement_level = determine_enforcement_level(landlock, seccomp, &namespaces);

    SandboxCapabilities {
        landlock,
        seccomp,
        namespaces,
        mount_isolation,
        enforcement_level,
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn check_seccomp_available() -> CapabilityLevel {
    if let Ok(contents) = std::fs::read_to_string("/proc/self/status") {
        for line in contents.lines() {
            if line.starts_with("Seccomp:") {
                return CapabilityLevel::Full;
            }
        }
    }
    CapabilityLevel::Full
}

#[cfg(target_os = "linux")]
pub(crate) fn check_namespace_capabilities() -> NamespaceCapability {
    let kernel = kernel_namespace_capabilities();
    let runtime = runtime_namespace_capabilities();

    NamespaceCapability {
        mount: kernel.mount && runtime.mount,
        pid: kernel.pid && runtime.pid,
        network: kernel.network && runtime.network,
        user: kernel.user && runtime.user,
    }
}

#[cfg(target_os = "linux")]
fn kernel_namespace_capabilities() -> NamespaceCapability {
    NamespaceCapability {
        mount: std::path::Path::new("/proc/self/ns/mnt").exists(),
        pid: std::path::Path::new("/proc/self/ns/pid").exists(),
        network: std::path::Path::new("/proc/self/ns/net").exists(),
        user: std::path::Path::new("/proc/self/ns/user").exists(),
    }
}

#[cfg(target_os = "linux")]
fn runtime_namespace_capabilities() -> NamespaceCapability {
    namespace::runtime_namespace_capabilities()
}

#[cfg(target_os = "linux")]
fn determine_enforcement_level(
    landlock: CapabilityLevel,
    seccomp: CapabilityLevel,
    namespaces: &NamespaceCapability,
) -> EnforcementLevel {
    match (landlock, seccomp, namespaces.any_available()) {
        (CapabilityLevel::Full, CapabilityLevel::Full, true) => EnforcementLevel::Enforced,
        (CapabilityLevel::Unavailable, _, false) | (_, CapabilityLevel::Unavailable, false) => {
            EnforcementLevel::Unsupported
        }
        _ => EnforcementLevel::Degraded,
    }
}

/// Check if strict policy can be enforced with current capabilities.
///
/// Returns Ok(()) if the policy can be fully enforced, or an error
/// describing why it cannot.
pub fn check_strict_policy_enforceable(
    policy: &Policy,
) -> Result<EnforcementLevel, SandboxSetupError> {
    check_policy_runtime_requirements(policy)?;
    let caps = detect_capabilities();

    #[cfg(not(target_os = "linux"))]
    {
        if !policy.landlock_optional && !policy.mount_isolation_fallback {
            return Err(SandboxSetupError::UnsupportedPlatform);
        }
        return Ok(EnforcementLevel::Unsupported);
    }

    #[cfg(target_os = "linux")]
    {
        check_strict_policy_enforceable_linux(policy, &caps)
    }
}

pub fn check_policy_runtime_requirements(policy: &Policy) -> Result<(), SandboxSetupError> {
    if let Some(network_policy) = &policy.network_policy {
        if network_policy.is_enabled() && !policy.allow_network() {
            return Err(SandboxSetupError::CapabilityError {
                feature: "network_policy".to_string(),
                reason: "network_policy is configured but namespaces.network = false blocks all network access"
                    .to_string(),
            });
        }

        if network_policy.is_enabled() {
            ensure_network_policy_backend_available(network_policy)?;
        }
    }

    Ok(())
}

pub(crate) fn ensure_network_policy_backend_available(
    network_policy: &NetworkPolicy,
) -> Result<(), SandboxSetupError> {
    if network_policy_backend_available() {
        return Ok(());
    }

    let reason = match network_policy.mode() {
        NetworkPolicyMode::Enforce => {
            "network_policy.mode = \"enforce\" requires the eBPF network enforcement backend, but it is not available in this runtime"
        }
        NetworkPolicyMode::Monitor => {
            "enabled network_policy requires the eBPF network monitoring backend, but it is not available in this runtime"
        }
    };

    Err(SandboxSetupError::CapabilityError {
        feature: "network_policy".to_string(),
        reason: reason.to_string(),
    })
}

fn network_policy_backend_available() -> bool {
    #[cfg(feature = "ebpf")]
    {
        true
    }
    #[cfg(not(feature = "ebpf"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn check_strict_policy_enforceable_linux(
    policy: &Policy,
    caps: &SandboxCapabilities,
) -> Result<EnforcementLevel, SandboxSetupError> {
    let (missing_critical_namespaces, missing_degraded_namespaces) =
        missing_requested_namespaces(&policy.namespaces, &caps.namespaces);
    if !missing_critical_namespaces.is_empty() {
        return Err(SandboxSetupError::CapabilityError {
            feature: "namespaces".to_string(),
            reason: format!(
                "requested Linux namespace isolation is not enforced by the active runtime: {}",
                missing_critical_namespaces.join(", ")
            ),
        });
    }

    let mut degraded = !missing_degraded_namespaces.is_empty();

    let fs_isolation_required = !policy.fs_rules().is_empty();
    let fs_isolation_available = caps.landlock == CapabilityLevel::Full
        || (policy.mount_isolation_fallback && caps.mount_isolation == CapabilityLevel::Full);

    if fs_isolation_required && !fs_isolation_available {
        if !policy.landlock_optional && !policy.mount_isolation_fallback {
            return Err(SandboxSetupError::LandlockSetupFailed(
                "Landlock unavailable and fallback not allowed by policy".to_string(),
            ));
        }

        degraded = true;
    }

    if fs_isolation_required && caps.landlock != CapabilityLevel::Full {
        degraded = true;
    }

    if caps.seccomp != CapabilityLevel::Full {
        degraded = true;
    }

    Ok(if degraded {
        EnforcementLevel::Degraded
    } else {
        EnforcementLevel::Enforced
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn missing_requested_namespaces(
    requested: &NamespaceConfig,
    available: &NamespaceCapability,
) -> (Vec<&'static str>, Vec<&'static str>) {
    let runtime_plan = requested.linux_runtime_plan();
    let mut critical = Vec::new();
    let mut degraded = Vec::new();

    if runtime_plan.user && !available.user {
        critical.push("user");
    }
    if runtime_plan.pid && !available.pid {
        critical.push("pid");
    }
    if runtime_plan.isolate_network && !available.network {
        critical.push("network");
    }
    if runtime_plan.mount && !available.mount {
        degraded.push("mount");
    }

    (critical, degraded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::NetworkPolicy;

    #[test]
    fn test_detect_capabilities() {
        let caps = detect_capabilities();

        #[cfg(target_os = "linux")]
        {
            let kernel = kernel_namespace_capabilities();
            let runtime = runtime_namespace_capabilities();

            assert_eq!(caps.namespaces.mount, kernel.mount && runtime.mount);
            assert_eq!(caps.namespaces.pid, kernel.pid && runtime.pid);
            assert_eq!(caps.namespaces.network, kernel.network && runtime.network);
            assert_eq!(caps.namespaces.user, kernel.user && runtime.user);
        }

        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(caps.landlock, CapabilityLevel::Unavailable);
            assert_eq!(caps.seccomp, CapabilityLevel::Unavailable);
            assert_eq!(caps.enforcement_level, EnforcementLevel::Unsupported);
        }
    }

    #[test]
    fn test_detect_capabilities_enforcement_level_consistency() {
        let caps = detect_capabilities();

        #[cfg(target_os = "linux")]
        {
            match caps.enforcement_level {
                EnforcementLevel::Enforced => {
                    assert_eq!(caps.landlock, CapabilityLevel::Full);
                    assert_eq!(caps.seccomp, CapabilityLevel::Full);
                    assert!(caps.namespaces.any_available());
                }
                EnforcementLevel::Degraded => {}
                EnforcementLevel::Unsupported => {}
            }
        }

        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(caps.enforcement_level, EnforcementLevel::Unsupported);
        }
    }

    #[test]
    fn test_check_policy_runtime_requirements_rejects_blocked_network_with_network_policy() {
        let mut policy = Policy::strict();
        policy.network_policy = Some(NetworkPolicy::default());

        let result = check_policy_runtime_requirements(&policy);

        assert!(matches!(
            result,
            Err(SandboxSetupError::CapabilityError { feature, reason })
                if feature == "network_policy"
                    && reason.contains("namespaces.network = false")
        ));
    }

    #[test]
    fn test_check_policy_runtime_requirements_accepts_disabled_network_policy_with_blocked_network()
    {
        let mut policy = Policy::strict();
        policy.network_policy = Some(NetworkPolicy {
            enabled: false,
            ..NetworkPolicy::default()
        });

        let result = check_policy_runtime_requirements(&policy);

        assert!(result.is_ok());
    }

    #[test]
    #[cfg(not(feature = "ebpf"))]
    fn test_check_policy_runtime_requirements_rejects_enabled_network_policy_without_backend() {
        let mut policy = Policy::with_network();
        policy.network_policy = Some(NetworkPolicy::default());

        let result = check_policy_runtime_requirements(&policy);

        assert!(matches!(
            result,
            Err(SandboxSetupError::CapabilityError { feature, reason })
                if feature == "network_policy"
                    && reason.contains("network monitoring backend")
        ));
    }

    #[test]
    #[cfg(not(feature = "ebpf"))]
    fn test_check_policy_runtime_requirements_rejects_enforce_mode_without_backend() {
        let mut policy = Policy::with_network();
        policy.network_policy = Some(NetworkPolicy {
            mode: Some(NetworkPolicyMode::Enforce),
            ..NetworkPolicy::default()
        });

        let result = check_policy_runtime_requirements(&policy);

        assert!(matches!(
            result,
            Err(SandboxSetupError::CapabilityError { feature, reason })
                if feature == "network_policy"
                    && reason.contains("mode = \"enforce\"")
                    && reason.contains("enforcement backend")
        ));
    }

    #[test]
    #[cfg(feature = "ebpf")]
    fn test_check_policy_runtime_requirements_accepts_enabled_network_policy_with_backend() {
        let mut policy = Policy::with_network();
        policy.network_policy = Some(NetworkPolicy::default());

        let result = check_policy_runtime_requirements(&policy);

        assert!(result.is_ok());
    }

    #[test]
    #[cfg(feature = "ebpf")]
    fn test_check_policy_runtime_requirements_accepts_enforce_mode_with_backend() {
        let mut policy = Policy::with_network();
        policy.network_policy = Some(NetworkPolicy {
            mode: Some(NetworkPolicyMode::Enforce),
            ..NetworkPolicy::default()
        });

        let result = check_policy_runtime_requirements(&policy);

        assert!(result.is_ok());
    }
}
