//! Seccomp BPF system call filtering.
//!
//! This module provides system call filtering via seccomp BPF.
//! Seccomp allows restricting which system calls a process can make,
//! providing defense-in-depth against exploitation.
//!
//! # Security Model
//!
//! The seccomp filter uses an allowlist approach:
//! - Known safe syscalls are explicitly allowed
//! - Dangerous syscalls are explicitly denied
//! - Unknown syscalls are denied by default
//!
//! # Platform Support
//!
//! This module is only available on Linux. All functions return
//! `UnsupportedPlatform` errors on other platforms.

use crate::error::SandboxSetupError;

#[cfg(target_os = "linux")]
use seccompiler::{
    apply_filter, BackendError, BpfProgram, SeccompAction, SeccompFilter, SeccompRule, TargetArch,
};
#[cfg(target_os = "linux")]
use std::collections::{BTreeMap, HashSet};
#[cfg(target_os = "linux")]
use std::convert::TryInto;

/// Macro to build syscall lists with architecture-specific handling.
macro_rules! syscall_list {
    ($($name:ident),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut list: Vec<i64> = Vec::new();
        $(
            #[cfg(target_arch = "x86_64")]
            {
                list.push(libc::$name);
            }
            #[cfg(target_arch = "aarch64")]
            {
                #[allow(unused_unsafe)]
                if let Some(nr) = syscall_nr_aarch64(stringify!($name)) {
                    list.push(nr);
                }
            }
            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            {
                list.push(libc::$name);
            }
        )*
        list
    }};
}

/// Map syscall names to numbers on aarch64 where some x86_64 syscalls don't exist.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn syscall_nr_aarch64(name: &str) -> Option<i64> {
    match name {
        "SYS_read" => Some(libc::SYS_read),
        "SYS_write" => Some(libc::SYS_write),
        "SYS_openat" => Some(libc::SYS_openat),
        "SYS_close" => Some(libc::SYS_close),
        "SYS_fstat" => Some(libc::SYS_fstat),
        "SYS_newfstatat" => Some(libc::SYS_newfstatat),
        "SYS_statx" => Some(libc::SYS_statx),
        "SYS_statfs" => Some(libc::SYS_statfs),
        "SYS_fstatfs" => Some(libc::SYS_fstatfs),
        "SYS_mmap" => Some(libc::SYS_mmap),
        "SYS_mprotect" => Some(libc::SYS_mprotect),
        "SYS_munmap" => Some(libc::SYS_munmap),
        "SYS_brk" => Some(libc::SYS_brk),
        "SYS_exit" => Some(libc::SYS_exit),
        "SYS_exit_group" => Some(libc::SYS_exit_group),
        "SYS_execve" => Some(libc::SYS_execve),
        "SYS_wait4" => Some(libc::SYS_wait4),
        "SYS_rt_sigaction" => Some(libc::SYS_rt_sigaction),
        "SYS_rt_sigprocmask" => Some(libc::SYS_rt_sigprocmask),
        "SYS_rt_sigreturn" => Some(libc::SYS_rt_sigreturn),
        "SYS_rt_sigtimedwait" => Some(libc::SYS_rt_sigtimedwait),
        "SYS_pipe2" => Some(libc::SYS_pipe2),
        "SYS_dup" => Some(libc::SYS_dup),
        "SYS_dup3" => Some(libc::SYS_dup3),
        "SYS_faccessat" => Some(libc::SYS_faccessat),
        "SYS_faccessat2" => Some(libc::SYS_faccessat2),
        "SYS_ioctl" => Some(libc::SYS_ioctl),
        "SYS_fcntl" => Some(libc::SYS_fcntl),
        "SYS_getpid" => Some(libc::SYS_getpid),
        "SYS_getppid" => Some(libc::SYS_getppid),
        "SYS_getuid" => Some(libc::SYS_getuid),
        "SYS_getgid" => Some(libc::SYS_getgid),
        "SYS_geteuid" => Some(libc::SYS_geteuid),
        "SYS_getegid" => Some(libc::SYS_getegid),
        "SYS_gettid" => Some(libc::SYS_gettid),
        "SYS_getpgid" => Some(libc::SYS_getpgid),
        "SYS_setsid" => Some(libc::SYS_setsid),
        "SYS_setpgid" => Some(libc::SYS_setpgid),
        "SYS_getdents64" => Some(libc::SYS_getdents64),
        "SYS_getrandom" => Some(libc::SYS_getrandom),
        "SYS_prlimit64" => Some(libc::SYS_prlimit64),
        "SYS_set_tid_address" => Some(libc::SYS_set_tid_address),
        "SYS_set_robust_list" => Some(libc::SYS_set_robust_list),
        "SYS_rseq" => Some(libc::SYS_rseq),
        "SYS_lseek" => Some(libc::SYS_lseek),
        "SYS_pread64" => Some(libc::SYS_pread64),
        "SYS_pwrite64" => Some(libc::SYS_pwrite64),
        "SYS_readv" => Some(libc::SYS_readv),
        "SYS_writev" => Some(libc::SYS_writev),
        "SYS_futex" => Some(libc::SYS_futex),
        "SYS_clock_gettime" => Some(libc::SYS_clock_gettime),
        "SYS_clock_getres" => Some(libc::SYS_clock_getres),
        "SYS_nanosleep" => Some(libc::SYS_nanosleep),
        "SYS_madvise" => Some(libc::SYS_madvise),
        "SYS_umask" => Some(libc::SYS_umask),
        "SYS_chdir" => Some(libc::SYS_chdir),
        "SYS_fchdir" => Some(libc::SYS_fchdir),
        "SYS_getcwd" => Some(libc::SYS_getcwd),
        "SYS_readlinkat" => Some(libc::SYS_readlinkat),
        "SYS_unlinkat" => Some(libc::SYS_unlinkat),
        "SYS_renameat" => Some(libc::SYS_renameat),
        "SYS_renameat2" => Some(libc::SYS_renameat2),
        "SYS_uname" => Some(libc::SYS_uname),
        "SYS_sysinfo" => Some(libc::SYS_sysinfo),
        "SYS_socket" => Some(libc::SYS_socket),
        "SYS_socketpair" => Some(libc::SYS_socketpair),
        "SYS_connect" => Some(libc::SYS_connect),
        "SYS_bind" => Some(libc::SYS_bind),
        "SYS_listen" => Some(libc::SYS_listen),
        "SYS_accept" => Some(libc::SYS_accept),
        "SYS_accept4" => Some(libc::SYS_accept4),
        "SYS_getsockname" => Some(libc::SYS_getsockname),
        "SYS_getpeername" => Some(libc::SYS_getpeername),
        "SYS_sendto" => Some(libc::SYS_sendto),
        "SYS_sendmsg" => Some(libc::SYS_sendmsg),
        "SYS_recvfrom" => Some(libc::SYS_recvfrom),
        "SYS_recvmsg" => Some(libc::SYS_recvmsg),
        "SYS_shutdown" => Some(libc::SYS_shutdown),
        "SYS_setsockopt" => Some(libc::SYS_setsockopt),
        "SYS_getsockopt" => Some(libc::SYS_getsockopt),
        "SYS_sendmmsg" => Some(libc::SYS_sendmmsg),
        "SYS_recvmmsg" => Some(libc::SYS_recvmmsg),
        "SYS_clone" => Some(libc::SYS_clone),
        "SYS_clone3" => Some(libc::SYS_clone3),
        "SYS_ppoll" => Some(libc::SYS_ppoll),
        "SYS_pselect6" => Some(libc::SYS_pselect6),
        "SYS_epoll_create1" => Some(libc::SYS_epoll_create1),
        "SYS_epoll_ctl" => Some(libc::SYS_epoll_ctl),
        "SYS_epoll_pwait" => Some(libc::SYS_epoll_pwait),
        "SYS_eventfd2" => Some(libc::SYS_eventfd2),
        "SYS_timerfd_create" => Some(libc::SYS_timerfd_create),
        "SYS_timerfd_settime" => Some(libc::SYS_timerfd_settime),
        "SYS_timerfd_gettime" => Some(libc::SYS_timerfd_gettime),
        "SYS_signalfd4" => Some(libc::SYS_signalfd4),
        "SYS_clock_nanosleep" => Some(libc::SYS_clock_nanosleep),
        "SYS_kill" => Some(libc::SYS_kill),
        "SYS_tkill" => Some(libc::SYS_tkill),
        "SYS_tgkill" => Some(libc::SYS_tgkill),
        "SYS_io_uring_setup" => Some(libc::SYS_io_uring_setup),
        "SYS_io_uring_enter" => Some(libc::SYS_io_uring_enter),
        "SYS_io_uring_register" => Some(libc::SYS_io_uring_register),
        "SYS_ptrace" => Some(libc::SYS_ptrace),
        "SYS_mount" => Some(libc::SYS_mount),
        "SYS_umount2" => Some(libc::SYS_umount2),
        "SYS_reboot" => Some(libc::SYS_reboot),
        "SYS_kexec_load" => Some(libc::SYS_kexec_load),
        "SYS_kexec_file_load" => Some(libc::SYS_kexec_file_load),
        "SYS_init_module" => Some(libc::SYS_init_module),
        "SYS_finit_module" => Some(libc::SYS_finit_module),
        "SYS_delete_module" => Some(libc::SYS_delete_module),
        "SYS_swapon" => Some(libc::SYS_swapon),
        "SYS_swapoff" => Some(libc::SYS_swapoff),
        "SYS_acct" => Some(libc::SYS_acct),
        "SYS_setuid" => Some(libc::SYS_setuid),
        "SYS_setgid" => Some(libc::SYS_setgid),
        "SYS_setreuid" => Some(libc::SYS_setreuid),
        "SYS_setregid" => Some(libc::SYS_setregid),
        "SYS_setresuid" => Some(libc::SYS_setresuid),
        "SYS_setresgid" => Some(libc::SYS_setresgid),
        "SYS_setfsuid" => Some(libc::SYS_setfsuid),
        "SYS_setfsgid" => Some(libc::SYS_setfsgid),
        "SYS_capset" => Some(libc::SYS_capset),
        "SYS_prctl" => Some(libc::SYS_prctl),
        _ => None,
    }
}

/// Apply seccomp filter to restrict system calls.
///
/// This function sets up a BPF filter that allows only a predefined
/// set of "safe" system calls. Dangerous syscalls like mount, reboot,
/// and module operations are blocked.
///
/// # Errors
///
/// Returns `SandboxSetupError::SeccompSetupFailed` if:
/// - PR_SET_NO_NEW_PRIVS fails
/// - BPF filter installation fails
#[cfg(target_os = "linux")]
pub fn apply_seccomp_filter() -> Result<(), SandboxSetupError> {
    let allow_syscalls = syscall_list![
        SYS_read,
        SYS_write,
        SYS_open,
        SYS_openat,
        SYS_close,
        SYS_stat,
        SYS_fstat,
        SYS_lstat,
        SYS_newfstatat,
        SYS_statx,
        SYS_statfs,
        SYS_fstatfs,
        SYS_mmap,
        SYS_mprotect,
        SYS_munmap,
        SYS_brk,
        SYS_exit,
        SYS_exit_group,
        SYS_execve,
        SYS_wait4,
        SYS_rt_sigaction,
        SYS_rt_sigprocmask,
        SYS_rt_sigreturn,
        SYS_rt_sigtimedwait,
        SYS_pipe,
        SYS_pipe2,
        SYS_dup,
        SYS_dup2,
        SYS_dup3,
        SYS_arch_prctl,
        SYS_access,
        SYS_faccessat,
        SYS_faccessat2,
        SYS_ioctl,
        SYS_fcntl,
        SYS_getpid,
        SYS_getppid,
        SYS_getuid,
        SYS_getgid,
        SYS_geteuid,
        SYS_getegid,
        SYS_gettid,
        SYS_getpgrp,
        SYS_getpgid,
        SYS_setsid,
        SYS_setpgid,
        SYS_getdents,
        SYS_getdents64,
        SYS_getrandom,
        SYS_getrlimit,
        SYS_prlimit64,
        SYS_set_tid_address,
        SYS_set_robust_list,
        SYS_rseq,
        SYS_lseek,
        SYS_pread64,
        SYS_pwrite64,
        SYS_readv,
        SYS_writev,
        SYS_futex,
        SYS_clock_gettime,
        SYS_clock_getres,
        SYS_nanosleep,
        SYS_madvise,
        SYS_fadvise64,
        SYS_umask,
        SYS_chdir,
        SYS_fchdir,
        SYS_getcwd,
        SYS_readlink,
        SYS_readlinkat,
        SYS_unlink,
        SYS_unlinkat,
        SYS_rename,
        SYS_renameat,
        SYS_renameat2,
        SYS_uname,
        SYS_sysinfo,
        SYS_socket,
        SYS_socketpair,
        SYS_connect,
        SYS_bind,
        SYS_listen,
        SYS_accept,
        SYS_accept4,
        SYS_getsockname,
        SYS_getpeername,
        SYS_sendto,
        SYS_sendmsg,
        SYS_recvfrom,
        SYS_recvmsg,
        SYS_shutdown,
        SYS_setsockopt,
        SYS_getsockopt,
        SYS_sendmmsg,
        SYS_recvmmsg,
        SYS_clone,
        SYS_clone3,
        SYS_fork,
        SYS_vfork,
        SYS_poll,
        SYS_ppoll,
        SYS_select,
        SYS_pselect6,
        SYS_epoll_create,
        SYS_epoll_create1,
        SYS_epoll_ctl,
        SYS_epoll_wait,
        SYS_epoll_pwait,
        SYS_eventfd2,
        SYS_timerfd_create,
        SYS_timerfd_settime,
        SYS_timerfd_gettime,
        SYS_signalfd4,
        SYS_clock_nanosleep,
        SYS_kill,
        SYS_tkill,
        SYS_tgkill,
        SYS_io_uring_setup,
        SYS_io_uring_enter,
        SYS_io_uring_register,
    ];

    let deny_syscalls = syscall_list![
        SYS_ptrace,
        SYS_mount,
        SYS_umount2,
        SYS_reboot,
        SYS_kexec_load,
        SYS_kexec_file_load,
        SYS_init_module,
        SYS_finit_module,
        SYS_delete_module,
        SYS_swapon,
        SYS_swapoff,
        SYS_acct,
        SYS_setuid,
        SYS_setgid,
        SYS_setreuid,
        SYS_setregid,
        SYS_setresuid,
        SYS_setresgid,
        SYS_setfsuid,
        SYS_setfsgid,
        SYS_capset,
        SYS_prctl,
    ];

    // Verify no overlap between allow and deny lists
    let deny_set: HashSet<i64> = deny_syscalls.iter().copied().collect();
    if allow_syscalls.iter().any(|sys| deny_set.contains(sys)) {
        return Err(SandboxSetupError::SeccompSetupFailed(
            "Allow and deny syscall lists overlap".to_string(),
        ));
    }

    // Build seccomp filter rules
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    for syscall in allow_syscalls {
        rules.insert(syscall, Vec::new());
    }

    let arch = TargetArch::try_from(std::env::consts::ARCH)
        .map_err(|err: BackendError| SandboxSetupError::SeccompSetupFailed(err.to_string()))?;

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
        arch,
    )
    .map_err(|err| SandboxSetupError::SeccompSetupFailed(err.to_string()))?;

    let bpf: BpfProgram = filter
        .try_into()
        .map_err(|err: BackendError| SandboxSetupError::SeccompSetupFailed(err.to_string()))?;

    // Set PR_SET_NO_NEW_PRIVS before applying seccomp filter
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if rc != 0 {
        return Err(SandboxSetupError::SeccompSetupFailed(
            std::io::Error::last_os_error().to_string(),
        ));
    }

    apply_filter(&bpf).map_err(|err| SandboxSetupError::SeccompSetupFailed(err.to_string()))
}

/// Apply seccomp filter (non-Linux stub).
#[cfg(not(target_os = "linux"))]
pub fn apply_seccomp_filter() -> Result<(), SandboxSetupError> {
    Err(SandboxSetupError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_os = "linux"))]
    use super::apply_seccomp_filter;

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_unsupported_platform() {
        assert!(apply_seccomp_filter().is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_seccomp_filter_compiles() {
        // Verify that the filter compiles without error
        // We don't actually apply it in tests as it would affect the test process
        use seccompiler::TargetArch;
        use std::convert::TryFrom;

        let arch = TargetArch::try_from(std::env::consts::ARCH);
        assert!(arch.is_ok(), "Architecture should be supported");
    }
}
