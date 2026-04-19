//! Environment and namespace utilities for sandbox isolation.
//!
//! This module provides utilities for managing process environment
//! variables and preparing for namespace-based isolation.

use crate::error::SandboxSetupError;
use crate::policy::NamespaceConfig;
use crate::sandbox::capability::NamespaceCapability;
use std::env;

#[cfg(target_os = "linux")]
use nix::mount::{mount, umount2, MntFlags, MsFlags};
#[cfg(target_os = "linux")]
use std::fs;

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
pub(crate) struct UserNamespaceSync {
    pub notify_parent_fd: libc::c_int,
    pub wait_parent_fd: libc::c_int,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NamespaceApplyResult {
    pub pid_fork_required: bool,
}

/// Clean and sanitize the environment variables.
///
/// This function clears all environment variables and then restores
/// only those in the whitelist. It also sets safe defaults for
/// HOME, USER, and PATH.
///
/// # Arguments
///
/// * `env_whitelist` - List of environment variable names to preserve
///
/// # Platform Support
///
/// This function is only effective on Linux. On other platforms,
/// it is a no-op.
#[cfg(target_os = "linux")]
pub fn clean_environment(env_whitelist: &[String]) {
    // Preserve whitelisted variables
    let mut preserved = Vec::new();
    for key in env_whitelist {
        if let Ok(value) = env::var(key) {
            preserved.push((key.clone(), value));
        }
    }

    // Clear all environment variables
    let clear_result = unsafe { libc::clearenv() };
    if clear_result != 0 {
        // Fallback: manually remove each variable
        for (key, _) in env::vars() {
            env::remove_var(&key);
        }
    }

    // Restore whitelisted variables
    for (key, value) in preserved {
        env::set_var(&key, &value);
    }

    // Set safe defaults
    env::set_var("HOME", "/tmp");
    env::set_var("USER", "nobody");
    env::set_var("PATH", "/usr/bin:/bin");
}

#[cfg(target_os = "linux")]
pub(crate) fn runtime_namespace_capabilities() -> NamespaceCapability {
    runtime_namespace_capabilities_with(probe_namespace_support)
}

#[cfg(target_os = "linux")]
pub(crate) fn apply_namespaces(
    config: &NamespaceConfig,
    user_sync: Option<UserNamespaceSync>,
) -> Result<NamespaceApplyResult, SandboxSetupError> {
    if config.user {
        unshare_namespace(libc::CLONE_NEWUSER, "user")?;
        setup_user_namespace(user_sync)?;
    }

    let flags = post_user_namespace_flags(config);
    if flags != 0 {
        unshare_namespace(flags, &post_user_namespace_description(config))?;
    }

    Ok(NamespaceApplyResult {
        pid_fork_required: config.pid,
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn remount_procfs() -> Result<(), SandboxSetupError> {
    let _ = umount2("/proc", MntFlags::MNT_DETACH);
    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::empty(),
        None::<&str>,
    )
    .map_err(|error| {
        SandboxSetupError::NamespaceSetupFailed(format!("failed to remount /proc: {error}"))
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn setup_parent_user_namespace(child_pid: libc::pid_t) -> Result<(), SandboxSetupError> {
    configure_user_namespace_maps(&child_pid.to_string())
}

#[cfg(target_os = "linux")]
fn runtime_namespace_capabilities_with<F>(probe: F) -> NamespaceCapability
where
    F: Fn(&NamespaceConfig) -> bool,
{
    let user = probe(&NamespaceConfig {
        mount: false,
        pid: false,
        network: true,
        user: true,
    });

    NamespaceCapability {
        mount: probe(&NamespaceConfig {
            mount: true,
            pid: false,
            network: true,
            user: false,
        }) || (user
            && probe(&NamespaceConfig {
                mount: true,
                pid: false,
                network: true,
                user: true,
            })),
        pid: probe(&NamespaceConfig {
            mount: false,
            pid: true,
            network: true,
            user: false,
        }) || (user
            && probe(&NamespaceConfig {
                mount: false,
                pid: true,
                network: true,
                user: true,
            })),
        network: probe(&NamespaceConfig {
            mount: false,
            pid: false,
            network: false,
            user: false,
        }) || (user
            && probe(&NamespaceConfig {
                mount: false,
                pid: false,
                network: false,
                user: true,
            })),
        user,
    }
}

#[cfg(target_os = "linux")]
fn probe_namespace_support(config: &NamespaceConfig) -> bool {
    let sync_pipes = if config.user {
        match ProbeUserNamespacePipes::new() {
            Some(pipes) => Some(pipes),
            None => return false,
        }
    } else {
        None
    };

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        if let Some(pipes) = sync_pipes {
            pipes.close_all();
        }
        return false;
    }

    if pid == 0 {
        if let Some(pipes) = sync_pipes {
            pipes.close_parent_ends();
        }

        let result = apply_namespaces(config, sync_pipes.map(|pipes| pipes.child_sync())).and_then(
            |applied| {
                if applied.pid_fork_required {
                    probe_pid_namespace_fork()
                } else {
                    Ok(())
                }
            },
        );

        if let Some(pipes) = sync_pipes {
            pipes.close_child_ends();
        }

        exit_probe_process(result.is_ok());
    }

    if let Some(pipes) = sync_pipes {
        pipes.close_child_ends();
        if complete_parent_user_namespace_sync(pid, pipes).is_err() {
            kill_probe_child(pid);
            pipes.close_parent_ends();
            return false;
        }
        pipes.close_parent_ends();
    }

    wait_for_probe_exit(pid)
}

#[cfg(target_os = "linux")]
fn setup_user_namespace(user_sync: Option<UserNamespaceSync>) -> Result<(), SandboxSetupError> {
    if let Some(user_sync) = user_sync {
        write_sync_byte(
            user_sync.notify_parent_fd,
            "user namespace parent notification",
        )?;
        read_sync_byte(
            user_sync.wait_parent_fd,
            "user namespace parent acknowledgement",
        )?;
        return Ok(());
    }

    configure_user_namespace_maps("self")
}

#[cfg(target_os = "linux")]
fn configure_user_namespace_maps(proc_target: &str) -> Result<(), SandboxSetupError> {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    let setgroups_path = format!("/proc/{proc_target}/setgroups");
    if fs::metadata(&setgroups_path).is_ok() {
        fs::write(&setgroups_path, "deny\n").map_err(|error| {
            SandboxSetupError::NamespaceSetupFailed(format!(
                "setgroups deny failed for {proc_target}: {error}"
            ))
        })?;
    }

    let uid_map_path = format!("/proc/{proc_target}/uid_map");
    fs::write(&uid_map_path, format!("0 {uid} 1\n")).map_err(|error| {
        SandboxSetupError::NamespaceSetupFailed(format!(
            "uid_map write failed for {proc_target}: {error}"
        ))
    })?;

    let gid_map_path = format!("/proc/{proc_target}/gid_map");
    fs::write(&gid_map_path, format!("0 {gid} 1\n")).map_err(|error| {
        SandboxSetupError::NamespaceSetupFailed(format!(
            "gid_map write failed for {proc_target}: {error}"
        ))
    })?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn post_user_namespace_flags(config: &NamespaceConfig) -> libc::c_int {
    let plan = config.linux_runtime_plan();
    let mut flags = 0;

    if plan.mount {
        flags |= libc::CLONE_NEWNS;
    }
    if plan.isolate_network {
        flags |= libc::CLONE_NEWNET;
    }
    if plan.pid {
        flags |= libc::CLONE_NEWPID;
    }

    flags
}

#[cfg(target_os = "linux")]
fn post_user_namespace_description(config: &NamespaceConfig) -> String {
    let plan = config.linux_runtime_plan();
    let mut names = Vec::new();

    if plan.mount {
        names.push("mount");
    }
    if plan.isolate_network {
        names.push("network");
    }
    if plan.pid {
        names.push("pid");
    }

    names.join(", ")
}

#[cfg(target_os = "linux")]
fn unshare_namespace(flags: libc::c_int, description: &str) -> Result<(), SandboxSetupError> {
    if unsafe { libc::unshare(flags) } != 0 {
        return Err(SandboxSetupError::NamespaceSetupFailed(format!(
            "unshare {description} namespace failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn write_sync_byte(fd: libc::c_int, action: &str) -> Result<(), SandboxSetupError> {
    let byte = [1_u8];
    let written = unsafe { libc::write(fd, byte.as_ptr() as *const libc::c_void, byte.len()) };
    if written == byte.len() as isize {
        return Ok(());
    }

    Err(SandboxSetupError::NamespaceSetupFailed(format!(
        "failed to write {action}: {}",
        std::io::Error::last_os_error()
    )))
}

#[cfg(target_os = "linux")]
fn read_sync_byte(fd: libc::c_int, action: &str) -> Result<(), SandboxSetupError> {
    let mut byte = [0_u8; 1];
    let read = unsafe { libc::read(fd, byte.as_mut_ptr() as *mut libc::c_void, byte.len()) };
    if read == byte.len() as isize {
        return Ok(());
    }

    let detail = if read == 0 {
        "pipe closed before data was received".to_string()
    } else {
        std::io::Error::last_os_error().to_string()
    };

    Err(SandboxSetupError::NamespaceSetupFailed(format!(
        "failed to read {action}: {detail}"
    )))
}

#[cfg(target_os = "linux")]
fn probe_pid_namespace_fork() -> Result<(), SandboxSetupError> {
    let grandchild = unsafe { libc::fork() };
    if grandchild < 0 {
        return Err(SandboxSetupError::NamespaceSetupFailed(format!(
            "fork after pid namespace unshare failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    if grandchild == 0 {
        unsafe {
            libc::_exit(0);
        }
    }

    let mut status = 0;
    if unsafe { libc::waitpid(grandchild, &mut status, 0) } != grandchild {
        return Err(SandboxSetupError::NamespaceSetupFailed(format!(
            "waitpid after pid namespace unshare failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
        return Ok(());
    }

    Err(SandboxSetupError::NamespaceSetupFailed(
        "pid namespace probe child exited unexpectedly".to_string(),
    ))
}

#[cfg(target_os = "linux")]
fn complete_parent_user_namespace_sync(
    child_pid: libc::pid_t,
    pipes: ProbeUserNamespacePipes,
) -> Result<(), SandboxSetupError> {
    read_sync_byte(
        pipes.child_to_parent[0],
        "probe child user namespace notification",
    )?;
    setup_parent_user_namespace(child_pid)?;
    write_sync_byte(
        pipes.parent_to_child[1],
        "probe child user namespace acknowledgement",
    )
}

#[cfg(target_os = "linux")]
fn wait_for_probe_exit(pid: libc::pid_t) -> bool {
    let mut status = 0;
    if unsafe { libc::waitpid(pid, &mut status, 0) } != pid {
        return false;
    }

    libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
}

#[cfg(target_os = "linux")]
fn kill_probe_child(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    let mut status = 0;
    unsafe {
        libc::waitpid(pid, &mut status, 0);
    }
}

#[cfg(target_os = "linux")]
fn exit_probe_process(success: bool) -> ! {
    unsafe {
        libc::_exit(if success { 0 } else { 1 });
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct ProbeUserNamespacePipes {
    child_to_parent: [libc::c_int; 2],
    parent_to_child: [libc::c_int; 2],
}

#[cfg(target_os = "linux")]
impl ProbeUserNamespacePipes {
    fn new() -> Option<Self> {
        let child_to_parent = create_pipe()?;
        let parent_to_child = match create_pipe() {
            Some(pipe) => pipe,
            None => {
                close_fd(child_to_parent[0]);
                close_fd(child_to_parent[1]);
                return None;
            }
        };

        Some(Self {
            child_to_parent,
            parent_to_child,
        })
    }

    fn child_sync(self) -> UserNamespaceSync {
        UserNamespaceSync {
            notify_parent_fd: self.child_to_parent[1],
            wait_parent_fd: self.parent_to_child[0],
        }
    }

    fn close_child_ends(self) {
        close_fd(self.child_to_parent[1]);
        close_fd(self.parent_to_child[0]);
    }

    fn close_parent_ends(self) {
        close_fd(self.child_to_parent[0]);
        close_fd(self.parent_to_child[1]);
    }

    fn close_all(self) {
        self.close_child_ends();
        self.close_parent_ends();
    }
}

#[cfg(target_os = "linux")]
fn create_pipe() -> Option<[libc::c_int; 2]> {
    let mut pipe = [0; 2];
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
        return None;
    }

    Some(pipe)
}

#[cfg(target_os = "linux")]
fn close_fd(fd: libc::c_int) {
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

/// Stub for non-Linux platforms - environment cleaning is not supported.
#[cfg(not(target_os = "linux"))]
pub fn clean_environment(_env_whitelist: &[String]) {
    // No-op on non-Linux platforms
    // The sandbox will return UnsupportedPlatform before this is called
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn test_clean_environment_preserves_whitelisted() {
        #[cfg(target_os = "linux")]
        {
            // Set a test variable
            env::set_var("TEST_PRESERVE", "test_value");

            // Clean environment with whitelist
            let whitelist = vec!["TEST_PRESERVE".to_string()];
            clean_environment(&whitelist);

            // Variable should be preserved
            assert_eq!(env::var("TEST_PRESERVE"), Ok("test_value".to_string()));

            // Cleanup
            env::remove_var("TEST_PRESERVE");
        }

        #[cfg(not(target_os = "linux"))]
        {
            // Just verify the function doesn't panic
            let whitelist = vec!["PATH".to_string()];
            clean_environment(&whitelist);
        }
    }

    #[test]
    #[serial_test::serial]
    fn test_clean_environment_sets_defaults() {
        #[cfg(target_os = "linux")]
        {
            let whitelist: Vec<String> = Vec::new();
            clean_environment(&whitelist);

            assert_eq!(env::var("HOME"), Ok("/tmp".to_string()));
            assert_eq!(env::var("USER"), Ok("nobody".to_string()));
            assert_eq!(env::var("PATH"), Ok("/usr/bin:/bin".to_string()));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_runtime_namespace_capabilities_use_user_assisted_probes() {
        let caps = runtime_namespace_capabilities_with(|config| {
            (config.user && !config.mount && !config.pid && config.network)
                || (config.user && config.mount && !config.pid && config.network)
        });

        assert!(caps.user);
        assert!(caps.mount);
        assert!(!caps.pid);
        assert!(!caps.network);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_runtime_namespace_capabilities_do_not_assume_user_assistance() {
        let caps = runtime_namespace_capabilities_with(|config| {
            config.mount && !config.user && !config.pid && config.network
        });

        assert!(caps.mount);
        assert!(!caps.user);
        assert!(!caps.pid);
        assert!(!caps.network);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_post_user_namespace_flags_exclude_user_namespace() {
        let flags = post_user_namespace_flags(&NamespaceConfig::full());

        assert_eq!(flags & libc::CLONE_NEWUSER, 0);
        assert_ne!(flags & libc::CLONE_NEWNS, 0);
        assert_ne!(flags & libc::CLONE_NEWPID, 0);
        assert_ne!(flags & libc::CLONE_NEWNET, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_post_user_namespace_flags_skip_network_namespace_when_network_is_allowed() {
        let flags = post_user_namespace_flags(&NamespaceConfig::without_user());

        assert_eq!(flags & libc::CLONE_NEWNET, 0);
        assert_ne!(flags & libc::CLONE_NEWNS, 0);
        assert_ne!(flags & libc::CLONE_NEWPID, 0);
    }
}
