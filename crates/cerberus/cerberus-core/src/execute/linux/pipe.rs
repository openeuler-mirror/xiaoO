//! Pipe management for Linux sandbox execution.

use crate::error::{CerberusError, ExecutionError};

/// Create a pipe with the given name for error reporting.
pub(super) fn create_pipe(name: &str) -> Result<[libc::c_int; 2], CerberusError> {
    let mut pipe = [0; 2];
    if unsafe { libc::pipe(pipe.as_mut_ptr()) } != 0 {
        return Err(ExecutionError::SpawnFailed(format!(
            "Failed to create {} pipe: {}",
            name,
            std::io::Error::last_os_error()
        ))
        .into());
    }

    Ok(pipe)
}

/// Set close-on-exec flag for a file descriptor.
pub(super) fn set_close_on_exec(fd: libc::c_int, name: &str) -> Result<(), CerberusError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(ExecutionError::SpawnFailed(format!(
            "Failed to read {} pipe flags: {}",
            name,
            std::io::Error::last_os_error()
        ))
        .into());
    }

    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } != 0 {
        return Err(ExecutionError::SpawnFailed(format!(
            "Failed to set close-on-exec for {} pipe: {}",
            name,
            std::io::Error::last_os_error()
        ))
        .into());
    }

    Ok(())
}

/// Close both ends of a pipe.
pub(super) fn close_pipe(pipe: [libc::c_int; 2]) {
    close_fd(pipe[0]);
    close_fd(pipe[1]);
}

/// Close a file descriptor.
pub(super) fn close_fd(fd: libc::c_int) {
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

/// Pipes for user namespace synchronization between parent and child.
#[derive(Debug, Clone, Copy)]
pub(super) struct UserNamespacePipes {
    pub(super) child_to_parent: [libc::c_int; 2],
    pub(super) parent_to_child: [libc::c_int; 2],
}

impl UserNamespacePipes {
    pub(super) fn new() -> Result<Self, CerberusError> {
        let child_to_parent = create_pipe("user namespace child-to-parent")?;
        let parent_to_child = match create_pipe("user namespace parent-to-child") {
            Ok(pipe) => pipe,
            Err(error) => {
                close_pipe(child_to_parent);
                return Err(error);
            }
        };

        Ok(Self {
            child_to_parent,
            parent_to_child,
        })
    }

    pub(super) fn child_sync(self) -> crate::sandbox::UserNamespaceSync {
        crate::sandbox::UserNamespaceSync {
            notify_parent_fd: self.child_to_parent[1],
            wait_parent_fd: self.parent_to_child[0],
        }
    }

    pub(super) fn close_child_ends(self) {
        close_fd(self.child_to_parent[1]);
        close_fd(self.parent_to_child[0]);
    }

    pub(super) fn close_parent_ends(self) {
        close_fd(self.child_to_parent[0]);
        close_fd(self.parent_to_child[1]);
    }

    pub(super) fn close_all(self) {
        self.close_child_ends();
        self.close_parent_ends();
    }
}

/// Spawn a thread to read from a pipe and collect the output.
pub(super) fn spawn_pipe_reader(fd: std::os::fd::RawFd) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        use std::fs::File;
        use std::io::Read;
        use std::os::fd::FromRawFd;

        let mut buffer = Vec::new();
        let mut file = unsafe { File::from_raw_fd(fd) };
        let _ = file.read_to_end(&mut buffer);
        buffer
    })
}

/// Join a pipe reader thread and return the collected output.
pub(super) fn join_pipe_reader(handle: std::thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    handle.join().unwrap_or_default()
}
