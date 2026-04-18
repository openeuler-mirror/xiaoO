//! User namespace synchronization between parent and child.

use super::pipe::UserNamespacePipes;
use crate::error::SandboxSetupError;

/// Set up user namespace from parent process.
pub(super) fn setup_user_namespace_from_parent(
    child_pid: libc::pid_t,
    pipes: UserNamespacePipes,
) -> Result<(), SandboxSetupError> {
    read_user_namespace_sync(
        pipes.child_to_parent[0],
        "child user namespace notification",
    )?;
    crate::sandbox::setup_parent_user_namespace(child_pid)?;
    write_user_namespace_sync(
        pipes.parent_to_child[1],
        "child user namespace acknowledgement",
    )
}

/// Read a synchronization byte from the child process.
pub(super) fn read_user_namespace_sync(
    fd: libc::c_int,
    action: &str,
) -> Result<(), SandboxSetupError> {
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

/// Write a synchronization byte to the child process.
pub(super) fn write_user_namespace_sync(
    fd: libc::c_int,
    action: &str,
) -> Result<(), SandboxSetupError> {
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
