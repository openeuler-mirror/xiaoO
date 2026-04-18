//! Public API for command execution.
//!
//! This module provides the main entry points for executing commands
//! with policy-based security controls.
//!
//! # Core Functions
//!
//! - [`execute`]: Execute a structured `ExecRequest` with a `Policy`
//! - [`execute_shell`]: Parse and execute a shell command string
//!
//! # Example
//!
//! ```rust,no_run
//! use cerberus_core::request::ExecRequest;
//! use cerberus_core::policy::Policy;
//! use cerberus_core::execute::execute;
//!
//! // Structured execution
//! let request = ExecRequest::new("ls")
//!     .arg("-la")
//!     .cwd("/tmp");
//! let policy = Policy::minimal();
//! let result = execute(request, &policy)?;
//!
//! // Shell string execution
//! use cerberus_core::execute::execute_shell;
//! let result = execute_shell("ls -la /tmp", &policy)?;
//! # Ok::<(), cerberus_core::CerberusError>(())
//! ```
//!
//! # Architecture
//!
//! The execute module is structured in layers:
//!
//! 1. **Public API** (`mod.rs`): Entry points `execute()` and `execute_shell()`
//! 2. **Preflight** (`preflight.rs`): Request validation before execution
//! 3. **Pipeline** (`pipeline.rs`): Orchestrates validation -> sandbox -> execution
//!
//! # Future Integration
//!
//! The pipeline includes a placeholder for sandbox integration.
//! The sandbox module is now integrated into the pipeline between preflight and execution.

mod env_filter;
#[cfg(target_os = "linux")]
mod linux;
mod pipeline;
mod preflight;

pub use preflight::validate;

use crate::error::{CerberusError, RequestError};
use crate::policy::Policy;
use crate::request::ExecRequest;
use crate::result::ExecResult;
use crate::{ExecObserver, ExecutionContext};

/// Execute a command with policy enforcement.
///
/// This is the primary entry point for command execution. It validates
/// the request, applies policy controls, and returns the execution result.
///
/// # Arguments
///
/// * `request` - The execution request containing program, args, cwd, env
/// * `policy` - Security policy controlling isolation and resource limits
///
/// # Returns
///
/// - `Ok(ExecResult)` on successful execution (including non-zero exit codes)
/// - `Err(CerberusError)` if validation, sandbox setup, or execution fails
///
/// # Example
///
/// ```rust,no_run
/// use cerberus_core::request::ExecRequest;
/// use cerberus_core::policy::Policy;
/// use cerberus_core::execute::execute;
///
/// let request = ExecRequest::new("echo")
///     .arg("hello world");
/// let policy = Policy::minimal();
///
/// let result = execute(request, &policy)?;
/// println!("Exit code: {}", result.exit_code);
/// println!("Output: {}", result.stdout_utf8());
/// # Ok::<(), cerberus_core::CerberusError>(())
/// ```
///
/// # Errors
///
/// - `CerberusError::Request`: Invalid request (empty program, bad cwd, etc.)
/// - `CerberusError::SandboxSetup`: Failed to set up isolation (future)
/// - `CerberusError::Execution`: Process spawn or wait failure
pub fn execute(request: ExecRequest, policy: &Policy) -> Result<ExecResult, CerberusError> {
    pipeline::run_pipeline(request, policy)
}

/// Execute a command with policy enforcement and execution audit observation.
pub fn execute_with_observer(
    request: ExecRequest,
    policy: &Policy,
    observer: Box<dyn ExecObserver>,
) -> Result<ExecResult, CerberusError> {
    let mut context = ExecutionContext::with_observer(&request, observer);
    pipeline::run_pipeline_with_context(request, policy, Some(&mut context))
}

/// Parse and execute a shell command string.
///
/// This convenience function parses a shell-style command string
/// into program and arguments, then executes with the given policy.
///
/// # Parsing Rules
///
/// The parsing follows shell-like quoting rules:
/// - Whitespace separates arguments
/// - Single quotes preserve literal content (no escaping)
/// - Double quotes preserve literal content (with `\"` escaping)
/// - Backslash escapes the next character
///
/// # Arguments
///
/// * `command` - Shell command string (e.g., `"ls -la /tmp"`)
/// * `policy` - Security policy controlling execution
///
/// # Returns
///
/// - `Ok(ExecResult)` on successful execution
/// - `Err(CerberusError::Request)` if parsing fails or command is empty
/// - Other errors as from [`execute`]
///
/// # Example
///
/// ```rust,no_run
/// use cerberus_core::policy::Policy;
/// use cerberus_core::execute::execute_shell;
///
/// let policy = Policy::minimal();
///
/// // Simple command
/// let result = execute_shell("ls -la", &policy)?;
///
/// // Quoted arguments
/// let result = execute_shell("echo 'hello world'", &policy)?;
///
/// // Complex quoting
/// let result = execute_shell(r#"grep "pattern" /path/to/file"#, &policy)?;
/// # Ok::<(), cerberus_core::CerberusError>(())
/// ```
///
/// # Security Note
///
/// This function does **not** invoke a shell. It parses the command
/// string and executes the program directly. This avoids shell injection
/// vulnerabilities.
pub fn execute_shell(command: &str, policy: &Policy) -> Result<ExecResult, CerberusError> {
    let (program, args) = parse_shell_command(command)?;

    // Check for empty program after parsing
    if program.is_empty() {
        return Err(RequestError::EmptyProgram.into());
    }

    let request = ExecRequest::new(program).args(args);
    execute(request, policy)
}

/// Parse and execute a shell command string with execution audit observation.
pub fn execute_shell_with_observer(
    command: &str,
    policy: &Policy,
    observer: Box<dyn ExecObserver>,
) -> Result<ExecResult, CerberusError> {
    let (program, args) = parse_shell_command(command)?;

    if program.is_empty() {
        return Err(RequestError::EmptyProgram.into());
    }

    let request = ExecRequest::new(program).args(args);
    execute_with_observer(request, policy, observer)
}

/// Parse and execute a shell command string, capturing stdout/stderr.
///
/// This is similar to [`execute_shell`] but captures output to buffers
/// instead of inheriting from the parent process.
pub fn execute_shell_capture(command: &str, policy: &Policy) -> Result<ExecResult, CerberusError> {
    let (program, args) = parse_shell_command(command)?;

    if program.is_empty() {
        return Err(RequestError::EmptyProgram.into());
    }

    let request = ExecRequest::new(program).args(args).capture_output();
    execute(request, policy)
}

/// Parses a shell command string into program and arguments.
///
/// Handles basic shell quoting:
/// - Whitespace separation
/// - Single quotes (literal, no escaping)
/// - Double quotes (with backslash escaping)
/// - Backslash escaping
fn parse_shell_command(command: &str) -> Result<(String, Vec<String>), RequestError> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            // Skip whitespace (separates arguments)
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            // Single-quoted string: preserve everything until closing quote
            '\'' => {
                chars.next(); // consume opening quote
                while let Some(&c) = chars.peek() {
                    if c == '\'' {
                        chars.next(); // consume closing quote
                        break;
                    }
                    current.push(chars.next().unwrap());
                }
            }
            // Double-quoted string: allow backslash escaping
            '"' => {
                chars.next(); // consume opening quote
                while let Some(&c) = chars.peek() {
                    match c {
                        '"' => {
                            chars.next(); // consume closing quote
                            break;
                        }
                        '\\' => {
                            chars.next(); // consume backslash
                            if let Some(&next) = chars.peek() {
                                if next == '"' || next == '\\' {
                                    current.push(chars.next().unwrap());
                                } else {
                                    current.push('\\');
                                }
                            }
                        }
                        _ => {
                            current.push(chars.next().unwrap());
                        }
                    }
                }
            }
            // Backslash escape outside quotes
            '\\' => {
                chars.next(); // consume backslash
                if chars.peek().is_some() {
                    current.push(chars.next().unwrap());
                }
            }
            // Regular character
            _ => {
                current.push(chars.next().unwrap());
            }
        }
    }

    // Push final argument if any
    if !current.is_empty() {
        args.push(current);
    }

    // First argument is the program, rest are args
    if args.is_empty() {
        return Ok((String::new(), Vec::new()));
    }

    let program = args.remove(0);
    Ok((program, args))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{EventType, LoggingObserver, MemorySink};

    #[test]
    fn execute_simple_command() {
        let request = ExecRequest::new("echo").arg("hello").capture_output();
        let policy = Policy::minimal();
        let result = execute(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello"));
    }

    #[test]
    fn execute_empty_program_fails() {
        let request = ExecRequest::new("");
        let policy = Policy::minimal();
        let result = execute(request, &policy);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CerberusError::Request(RequestError::EmptyProgram)
        ));
    }

    #[test]
    fn execute_invalid_cwd_fails() {
        let request = ExecRequest::new("ls").cwd("/nonexistent/path");
        let policy = Policy::minimal();
        let result = execute(request, &policy);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CerberusError::Request(RequestError::InvalidWorkingDirectory(_))
        ));
    }

    #[test]
    fn execute_shell_simple() {
        let policy = Policy::minimal();
        let result =
            execute_shell_capture("echo hello world", &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello world"));
    }

    #[test]
    fn execute_shell_empty_command_fails() {
        let policy = Policy::minimal();
        let result = execute_shell("", &policy);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CerberusError::Request(RequestError::EmptyProgram)
        ));
    }

    #[test]
    fn execute_shell_whitespace_only_command_fails() {
        let policy = Policy::minimal();
        let result = execute_shell("   ", &policy);

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CerberusError::Request(RequestError::EmptyProgram)
        ));
    }

    #[test]
    fn execute_shell_with_single_quotes() {
        let policy = Policy::minimal();
        let result =
            execute_shell_capture("echo 'hello world'", &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello world"));
    }

    #[test]
    fn execute_shell_with_double_quotes() {
        let policy = Policy::minimal();
        let result = execute_shell_capture(r#"echo "hello world""#, &policy)
            .expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello world"));
    }

    #[test]
    fn execute_shell_with_escaped_chars() {
        let policy = Policy::minimal();
        let result = execute_shell_capture(r#"echo hello\ world"#, &policy)
            .expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello world"));
    }

    #[test]
    fn parse_shell_command_simple() {
        let (program, args) = parse_shell_command("ls -la /tmp").unwrap();
        assert_eq!(program, "ls");
        assert_eq!(args, vec!["-la", "/tmp"]);
    }

    #[test]
    fn parse_shell_command_empty() {
        let (program, args) = parse_shell_command("").unwrap();
        assert!(program.is_empty());
        assert!(args.is_empty());
    }

    #[test]
    fn parse_shell_command_whitespace_only() {
        let (program, args) = parse_shell_command("   ").unwrap();
        assert!(program.is_empty());
        assert!(args.is_empty());
    }

    #[test]
    fn parse_shell_command_single_quotes() {
        let (program, args) = parse_shell_command("echo 'hello world'").unwrap();
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["hello world"]);
    }

    #[test]
    fn parse_shell_command_double_quotes() {
        let (program, args) = parse_shell_command(r#"echo "hello world""#).unwrap();
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["hello world"]);
    }

    #[test]
    fn parse_shell_command_escaped_quote() {
        let (program, args) = parse_shell_command(r#"echo "hello \"world\"""#).unwrap();
        assert_eq!(program, "echo");
        assert_eq!(args, vec![r#"hello "world""#]);
    }

    #[test]
    fn parse_shell_command_backslash_escape() {
        let (program, args) = parse_shell_command(r#"echo hello\ world"#).unwrap();
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["hello world"]);
    }

    #[test]
    fn parse_shell_command_mixed_quotes() {
        let (program, args) = parse_shell_command(r#"echo 'single' "double""#).unwrap();
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["single", "double"]);
    }

    #[test]
    fn parse_shell_command_unclosed_quote() {
        // Unclosed quotes are handled gracefully - content is captured until end
        let (program, args) = parse_shell_command("echo 'unclosed").unwrap();
        assert_eq!(program, "echo");
        assert_eq!(args, vec!["unclosed"]);
    }

    #[test]
    fn parse_shell_command_multiple_spaces() {
        let (program, args) = parse_shell_command("ls   -la    /tmp").unwrap();
        assert_eq!(program, "ls");
        assert_eq!(args, vec!["-la", "/tmp"]);
    }

    #[test]
    fn parse_shell_command_newline_separator() {
        let (program, args) = parse_shell_command("ls\n-la").unwrap();
        assert_eq!(program, "ls");
        assert_eq!(args, vec!["-la"]);
    }

    #[test]
    fn execute_with_policy_timeout() {
        // Create a policy with a short timeout
        let mut policy = Policy::minimal();
        policy.resources.timeout_secs = 1;

        // Command that sleeps for longer than timeout
        let request = ExecRequest::new("sleep").arg("10");
        let result = execute(request, &policy).expect("execution should complete");

        assert!(result.metadata.timed_out);
        assert!(result.metadata.killed);
    }

    #[test]
    fn execute_with_observer_emits_timeout_event() {
        let sink = MemorySink::new();
        let observer = LoggingObserver::new(Box::new(sink.clone()));

        let mut policy = Policy::minimal();
        policy.resources.timeout_secs = 1;

        let request = ExecRequest::new("sleep").arg("10");
        let result = execute_with_observer(request, &policy, Box::new(observer))
            .expect("execution should complete");

        assert!(result.metadata.timed_out);

        let events = sink.events().unwrap();
        assert!(events
            .iter()
            .any(|event| event.event_type == EventType::ExecutionTimedOut.as_str()));
        assert!(!events
            .iter()
            .any(|event| event.event_type == EventType::ExecutionCompleted.as_str()));
    }
}
