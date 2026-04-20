//! Linux sandbox execution.

use super::error::{child_sandbox_error, child_spawn_error, read_child_error};
use super::namespace_sync::setup_user_namespace_from_parent;
use super::pipe::{
    close_fd, close_pipe, create_pipe, join_pipe_reader, set_close_on_exec, spawn_pipe_reader,
    UserNamespacePipes,
};
use super::wait::{kill_and_reap, wait_pid, wait_pid_timeout};
use crate::audit::{ExecutionContext, TimeoutEvent};
use crate::error::{CerberusError, ExecutionError, SandboxSetupError};
use crate::policy::Policy;
use crate::request::{ExecRequest, OutputPolicy, StdinPolicy};
use crate::result::{ExecMetadata, ExecResult};
use crate::sandbox::SandboxSetup;
use std::fs::File;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::process::ExitStatusExt;
use std::time::{Instant, SystemTime};

/// Execute a process with Linux sandbox isolation.
pub(in crate::execute) fn execute_process_linux(
    request: &ExecRequest,
    policy: &Policy,
    context: Option<&ExecutionContext>,
) -> Result<ExecResult, CerberusError> {
    use crate::sandbox::check_strict_policy_enforceable;

    // Pre-execution capability check: fail closed for strict policies
    if crate::execute::pipeline::should_use_linux_sandbox(policy) {
        if let Err(e) = check_strict_policy_enforceable(policy) {
            return Err(CerberusError::SandboxSetup(e));
        }
    }

    let start = Instant::now();
    let stdout_pipe = if matches!(request.stdout, OutputPolicy::Capture) {
        Some(create_pipe("stdout")?)
    } else {
        None
    };
    let stderr_pipe = if matches!(request.stderr, OutputPolicy::Capture) {
        Some(create_pipe("stderr")?)
    } else {
        None
    };
    let error_pipe = create_pipe("error")?;
    set_close_on_exec(error_pipe[1], "error")?;
    let stdin_pipe = if matches!(request.stdin, StdinPolicy::Bytes(_)) {
        Some(create_pipe("stdin")?)
    } else {
        None
    };
    let user_namespace_pipes = if policy.namespaces.user {
        Some(UserNamespacePipes::new()?)
    } else {
        None
    };

    #[cfg(feature = "ebpf")]
    let _network_enforcement_handle = if policy.allow_network() {
        if let Some(np) = &policy.network_policy {
            if np.is_enabled() {
                match crate::sandbox::network::initialize_network_enforcement(np.clone()) {
                    Ok(handle) => Some(handle),
                    Err(e) => {
                        return Err(CerberusError::SandboxSetup(
                            SandboxSetupError::EbpfSetupFailed(format!(
                                "Network enforcement initialization failed: {}",
                                e
                            )),
                        ));
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    #[cfg(not(feature = "ebpf"))]
    if policy.allow_network()
        && policy
            .network_policy
            .as_ref()
            .map(|np| np.is_enabled())
            .unwrap_or(false)
    {
        return Err(CerberusError::SandboxSetup(
            SandboxSetupError::EbpfSetupFailed(
                "Network policy configured but eBPF support is disabled".to_string(),
            ),
        ));
    }

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        if let Some(pipe) = stdout_pipe {
            close_pipe(pipe);
        }
        if let Some(pipe) = stderr_pipe {
            close_pipe(pipe);
        }
        close_pipe(error_pipe);
        if let Some(pipe) = stdin_pipe {
            close_pipe(pipe);
        }
        if let Some(pipes) = user_namespace_pipes {
            pipes.close_all();
        }

        return Err(ExecutionError::SpawnFailed(format!(
            "Failed to fork '{}': {}",
            request.program,
            std::io::Error::last_os_error()
        ))
        .into());
    }

    if pid == 0 {
        run_sandboxed_child(
            request,
            policy,
            stdout_pipe,
            stderr_pipe,
            stdin_pipe,
            error_pipe,
            user_namespace_pipes,
        );
    }

    if let Some(pipe) = stdout_pipe {
        close_fd(pipe[1]);
    }
    if let Some(pipe) = stderr_pipe {
        close_fd(pipe[1]);
    }
    close_fd(error_pipe[1]);
    if let Some(pipe) = stdin_pipe {
        close_fd(pipe[0]);
    }
    if let Some(pipes) = user_namespace_pipes {
        pipes.close_child_ends();
        if let Err(error) = setup_user_namespace_from_parent(pid, pipes) {
            kill_and_reap(pid);
            if let Some(pipe) = stdout_pipe {
                close_fd(pipe[0]);
            }
            if let Some(pipe) = stderr_pipe {
                close_fd(pipe[0]);
            }
            if let Some(pipe) = stdin_pipe {
                close_fd(pipe[1]);
            }

            return match read_child_error(error_pipe[0])? {
                Some(child_error) => Err(child_error),
                None => Err(CerberusError::SandboxSetup(error)),
            };
        }
        pipes.close_parent_ends();
    }

    let stdout_reader = stdout_pipe.map(|pipe| spawn_pipe_reader(pipe[0]));
    let stderr_reader = stderr_pipe.map(|pipe| spawn_pipe_reader(pipe[0]));

    let stdin_write_error = match (&request.stdin, stdin_pipe) {
        (StdinPolicy::Bytes(data), Some(pipe)) => {
            let result = {
                let mut stdin = unsafe { File::from_raw_fd(pipe[1]) };
                stdin.write_all(data).map_err(|error| error.to_string())
            };
            result.err()
        }
        _ => None,
    };

    let timeout = policy.timeout();
    let exit_status = if timeout.as_secs() > 0 {
        match wait_pid_timeout(pid, timeout)? {
            Some(status) => status,
            None => {
                kill_and_reap(pid);
                if let Some(reader) = stdout_reader {
                    let _ = join_pipe_reader(reader);
                }
                if let Some(reader) = stderr_reader {
                    let _ = join_pipe_reader(reader);
                }
                close_fd(error_pipe[0]);

                if let Some(ctx) = context {
                    ctx.emit_timeout(&TimeoutEvent {
                        duration: timeout,
                        pid: pid as u32,
                        timestamp: SystemTime::now(),
                    })?;
                }

                return Ok(ExecResult::new(-1)
                    .duration(start.elapsed())
                    .metadata(ExecMetadata::new().timed_out().killed()));
            }
        }
    } else {
        wait_pid(pid)?
    };

    let duration = start.elapsed();
    let stdout_bytes = stdout_reader.map_or(Vec::new(), join_pipe_reader);
    let stderr_bytes = stderr_reader.map_or(Vec::new(), join_pipe_reader);

    if let Some(child_error) = read_child_error(error_pipe[0])? {
        return Err(child_error);
    }

    if let Some(error) = stdin_write_error {
        return Err(
            ExecutionError::SpawnFailed(format!("Failed to write stdin: {}", error)).into(),
        );
    }

    let exit_code = exit_status.code().unwrap_or(-1);
    let metadata = if let Some(signal) = exit_status.signal() {
        ExecMetadata::new().killed().signal(signal)
    } else {
        ExecMetadata::new()
    };

    Ok(ExecResult::new(exit_code)
        .stdout(stdout_bytes)
        .stderr(stderr_bytes)
        .duration(duration)
        .metadata(metadata))
}

/// Run the sandboxed child process (called in child after fork).
fn run_sandboxed_child(
    request: &ExecRequest,
    policy: &Policy,
    stdout_pipe: Option<[libc::c_int; 2]>,
    stderr_pipe: Option<[libc::c_int; 2]>,
    stdin_pipe: Option<[libc::c_int; 2]>,
    error_pipe: [libc::c_int; 2],
    user_namespace_pipes: Option<UserNamespacePipes>,
) -> ! {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    if let Some(pipe) = stdout_pipe {
        close_fd(pipe[0]);
    }
    if let Some(pipe) = stderr_pipe {
        close_fd(pipe[0]);
    }
    close_fd(error_pipe[0]);
    if let Some(pipes) = user_namespace_pipes {
        pipes.close_parent_ends();
    }

    set_child_process_group(error_pipe[1]);

    if let Some(pipe) = stdout_pipe {
        dup2_or_exit(pipe[1], libc::STDOUT_FILENO, error_pipe[1], "stdout");
    }
    if let Some(pipe) = stderr_pipe {
        dup2_or_exit(pipe[1], libc::STDERR_FILENO, error_pipe[1], "stderr");
    }

    match (&request.stdin, stdin_pipe) {
        (StdinPolicy::Bytes(_), Some(pipe)) => {
            close_fd(pipe[1]);
            dup2_or_exit(pipe[0], libc::STDIN_FILENO, error_pipe[1], "stdin");
            close_fd(pipe[0]);
        }
        (StdinPolicy::Null, _) => {
            let dev_null_path = b"/dev/null\0";
            let dev_null = unsafe {
                libc::open(
                    dev_null_path.as_ptr() as *const libc::c_char,
                    libc::O_RDONLY,
                )
            };
            if dev_null < 0 {
                child_spawn_error(
                    error_pipe[1],
                    &format!(
                        "Failed to open /dev/null: {}",
                        std::io::Error::last_os_error()
                    ),
                );
            }

            dup2_or_exit(dev_null, libc::STDIN_FILENO, error_pipe[1], "stdin");
            close_fd(dev_null);
        }
        _ => {}
    }

    if let Some(pipe) = stdout_pipe {
        close_fd(pipe[1]);
    }
    if let Some(pipe) = stderr_pipe {
        close_fd(pipe[1]);
    }

    let sandbox = SandboxSetup::new(policy);
    if let Err(error) =
        sandbox.setup_with_namespace_sync(user_namespace_pipes.map(|pipes| pipes.child_sync()))
    {
        child_sandbox_error(error_pipe[1], &error);
    }

    if let Err(error) = apply_child_environment(&request.env) {
        child_spawn_error(error_pipe[1], &error);
    }

    if let Some(cwd) = &request.cwd {
        let cwd = match CString::new(cwd.as_os_str().as_bytes()) {
            Ok(cwd) => cwd,
            Err(_) => child_spawn_error(
                error_pipe[1],
                &format!("Working directory contains null byte: {}", cwd.display()),
            ),
        };

        if unsafe { libc::chdir(cwd.as_ptr()) } != 0 {
            child_spawn_error(
                error_pipe[1],
                &format!(
                    "Failed to set current directory to '{}': {}",
                    cwd.to_string_lossy(),
                    std::io::Error::last_os_error()
                ),
            );
        }
    }

    let program = match CString::new(request.program.as_str()) {
        Ok(program) => program,
        Err(_) => child_spawn_error(error_pipe[1], "Program contains null byte"),
    };

    let args: Vec<CString> = request
        .args
        .iter()
        .map(|arg| CString::new(arg.as_str()))
        .collect::<Result<_, _>>()
        .unwrap_or_else(|_| child_spawn_error(error_pipe[1], "Argument contains null byte"));

    let argv: Vec<*const libc::c_char> = std::iter::once(program.as_ptr())
        .chain(args.iter().map(|arg| arg.as_ptr()))
        .chain(std::iter::once(std::ptr::null()))
        .collect();

    unsafe {
        libc::execvp(program.as_ptr(), argv.as_ptr());
    }

    child_spawn_error(
        error_pipe[1],
        &format!(
            "Failed to spawn '{}': {}",
            request.program,
            std::io::Error::last_os_error()
        ),
    );
}

/// Set the child process to its own process group.
fn set_child_process_group(error_fd: libc::c_int) {
    if unsafe { libc::setpgid(0, 0) } != 0 {
        child_spawn_error(
            error_fd,
            &format!(
                "Failed to place sandbox child in its own process group: {}",
                std::io::Error::last_os_error()
            ),
        );
    }
}

/// Duplicate a file descriptor or exit with error.
fn dup2_or_exit(from_fd: libc::c_int, to_fd: libc::c_int, error_fd: libc::c_int, name: &str) {
    if unsafe { libc::dup2(from_fd, to_fd) } < 0 {
        child_spawn_error(
            error_fd,
            &format!(
                "Failed to redirect {}: {}",
                name,
                std::io::Error::last_os_error()
            ),
        );
    }
}

/// Apply environment variables in the child process.
fn apply_child_environment(env: &[(String, String)]) -> Result<(), String> {
    use std::ffi::CString;

    if unsafe { libc::clearenv() } != 0 {
        return Err(format!(
            "Failed to clear child environment: {}",
            std::io::Error::last_os_error()
        ));
    }

    for (key, value) in env {
        let key_cstr = CString::new(key.as_str())
            .map_err(|_| format!("Environment variable '{}' contains null byte", key))?;
        let value_cstr = CString::new(value.as_str())
            .map_err(|_| format!("Environment value for '{}' contains null byte", key))?;

        if unsafe { libc::setenv(key_cstr.as_ptr(), value_cstr.as_ptr(), 1) } != 0 {
            return Err(format!(
                "Failed to set environment variable '{}': {}",
                key,
                std::io::Error::last_os_error()
            ));
        }
    }

    Ok(())
}
