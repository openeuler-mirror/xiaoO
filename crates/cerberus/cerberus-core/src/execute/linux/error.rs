//! Child process error handling.

use crate::error::{CerberusError, ExecutionError, SandboxSetupError};

/// Read error message from child process via pipe.
pub(super) fn read_child_error(fd: libc::c_int) -> Result<Option<CerberusError>, CerberusError> {
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::FromRawFd;

    let mut buffer = Vec::new();
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.read_to_end(&mut buffer)
        .map_err(|error| ExecutionError::WaitFailed(error.to_string()))?;

    if buffer.is_empty() {
        return Ok(None);
    }

    let message = String::from_utf8_lossy(&buffer);
    Ok(Some(parse_child_error(&message)))
}

/// Parse error message from child process.
pub(super) fn parse_child_error(message: &str) -> CerberusError {
    if let Some(message) = message.strip_prefix("spawn:") {
        return ExecutionError::SpawnFailed(message.to_string()).into();
    }

    if let Some(message) = message.strip_prefix("sandbox:") {
        let mut parts = message.splitn(3, ':');
        let kind = parts.next().unwrap_or_default();
        let detail = parts.next().unwrap_or_default().to_string();
        let rest = parts.next().unwrap_or_default().to_string();
        let error = match kind {
            "unsupported" => SandboxSetupError::UnsupportedPlatform,
            "namespace" => SandboxSetupError::NamespaceSetupFailed(detail),
            "landlock" => SandboxSetupError::LandlockSetupFailed(detail),
            "seccomp" => SandboxSetupError::SeccompSetupFailed(detail),
            "mount" => SandboxSetupError::MountIsolationFailed(detail),
            "ebpf" => SandboxSetupError::EbpfSetupFailed(detail),
            "capability" => SandboxSetupError::CapabilityError {
                feature: detail,
                reason: rest,
            },
            _ => SandboxSetupError::NamespaceSetupFailed(message.to_string()),
        };

        return error.into();
    }

    ExecutionError::SpawnFailed(message.to_string()).into()
}

/// Report a spawn error from child process and exit.
pub(super) fn child_spawn_error(error_fd: libc::c_int, message: &str) -> ! {
    write_child_error(error_fd, &format!("spawn:{}", message));
}

/// Report a sandbox setup error from child process and exit.
pub(super) fn child_sandbox_error(error_fd: libc::c_int, error: &SandboxSetupError) -> ! {
    let message = match error {
        SandboxSetupError::UnsupportedPlatform => "sandbox:unsupported".to_string(),
        SandboxSetupError::NamespaceSetupFailed(detail) => {
            format!("sandbox:namespace:{}", detail)
        }
        SandboxSetupError::LandlockSetupFailed(detail) => {
            format!("sandbox:landlock:{}", detail)
        }
        SandboxSetupError::SeccompSetupFailed(detail) => {
            format!("sandbox:seccomp:{}", detail)
        }
        SandboxSetupError::MountIsolationFailed(detail) => {
            format!("sandbox:mount:{}", detail)
        }
        SandboxSetupError::EbpfSetupFailed(detail) => format!("sandbox:ebpf:{}", detail),
        SandboxSetupError::CapabilityError { feature, reason } => {
            format!("sandbox:capability:{}:{}", feature, reason)
        }
    };

    write_child_error(error_fd, &message);
}

/// Write error message to parent via pipe and exit.
fn write_child_error(error_fd: libc::c_int, message: &str) -> ! {
    let _ = unsafe {
        libc::write(
            error_fd,
            message.as_ptr() as *const libc::c_void,
            message.len(),
        )
    };

    unsafe {
        libc::_exit(1);
    }
}
