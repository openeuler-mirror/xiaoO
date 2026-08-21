//! Process waiting and termination.

use crate::error::{CerberusError, ExecutionError};
use std::time::Instant;

/// Wait for a process with a timeout.
pub(super) fn wait_pid_timeout(
    pid: libc::pid_t,
    timeout: std::time::Duration,
) -> Result<std::option::Option<std::process::ExitStatus>, CerberusError> {
    use std::os::unix::process::ExitStatusExt;

    let start = Instant::now();
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };

        if result == pid {
            return Ok(Some(std::process::ExitStatus::from_raw(status)));
        }

        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }

            return Err(ExecutionError::WaitFailed(error.to_string()).into());
        }

        if start.elapsed() >= timeout {
            return Ok(None);
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Wait for a process to exit.
pub(super) fn wait_pid(pid: libc::pid_t) -> Result<std::process::ExitStatus, CerberusError> {
    use std::os::unix::process::ExitStatusExt;

    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid {
            return Ok(std::process::ExitStatus::from_raw(status));
        }

        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }

            return Err(ExecutionError::WaitFailed(error.to_string()).into());
        }
    }
}

/// Kill a process and reap its exit status.
pub(super) fn kill_and_reap(pid: libc::pid_t) {
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
        libc::kill(pid, libc::SIGKILL);
    }

    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid || result < 0 {
            break;
        }
    }
}
