//! Preflight validation for execution requests.
//!
//! This module performs validation checks before any execution attempt.
//! Preflight validation catches invalid requests early, providing
//! clear error messages before sandbox setup or process spawning.
//!
//! # Validation Steps
//!
//! 1. Program path is not empty
//! 2. Working directory exists (if specified)
//! 3. Environment variable format is valid (KEY=VALUE style or (key, value) tuples)

use crate::error::RequestError;
use crate::request::ExecRequest;
use std::path::Path;

/// Validates an execution request before processing.
///
/// This function performs all preflight checks to ensure the request
/// is well-formed before any expensive operations (like sandbox setup).
///
/// # Errors
///
/// Returns `RequestError` if:
/// - Program path is empty
/// - Specified working directory does not exist
/// - Environment variable format is invalid
pub fn validate(request: &ExecRequest) -> Result<(), RequestError> {
    validate_program(&request.program)?;
    validate_working_directory(request.cwd.as_deref())?;
    validate_environment(&request.env)?;
    Ok(())
}

/// Validates that the program path is not empty.
fn validate_program(program: &str) -> Result<(), RequestError> {
    if program.trim().is_empty() {
        return Err(RequestError::EmptyProgram);
    }
    Ok(())
}

/// Validates that the working directory exists if specified.
fn validate_working_directory(cwd: Option<&Path>) -> Result<(), RequestError> {
    if let Some(path) = cwd {
        if !path.exists() {
            return Err(RequestError::InvalidWorkingDirectory(path.to_path_buf()));
        }
        if !path.is_dir() {
            return Err(RequestError::InvalidWorkingDirectory(path.to_path_buf()));
        }
    }
    Ok(())
}

/// Validates environment variable format.
///
/// Checks that:
/// - Keys are not empty
/// - Keys contain only valid characters (alphanumeric and underscore, not starting with digit)
fn validate_environment(env: &[(String, String)]) -> Result<(), RequestError> {
    for (key, _value) in env {
        if key.is_empty() {
            return Err(RequestError::InvalidEnvironment(
                "Empty environment variable key".to_string(),
            ));
        }

        // First character must be letter or underscore
        let first_char = key.chars().next();
        if let Some(c) = first_char {
            if !c.is_ascii_alphabetic() && c != '_' {
                return Err(RequestError::InvalidEnvironment(format!(
                    "Invalid environment variable key '{}': must start with letter or underscore",
                    key
                )));
            }
        }

        // Remaining characters must be alphanumeric or underscore
        for c in key.chars().skip(1) {
            if !c.is_ascii_alphanumeric() && c != '_' {
                return Err(RequestError::InvalidEnvironment(format!(
                    "Invalid environment variable key '{}': contains invalid character '{}'",
                    key, c
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_valid_request() {
        let request = ExecRequest::new("ls").arg("-la");
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn validate_rejects_empty_program() {
        let request = ExecRequest::new("");
        let result = validate(&request);
        assert!(matches!(result, Err(RequestError::EmptyProgram)));
    }

    #[test]
    fn validate_rejects_whitespace_only_program() {
        let request = ExecRequest::new("   ");
        let result = validate(&request);
        assert!(matches!(result, Err(RequestError::EmptyProgram)));
    }

    #[test]
    fn validate_accepts_valid_working_directory() {
        let request = ExecRequest::new("ls").cwd("/tmp");
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn validate_rejects_nonexistent_working_directory() {
        let request = ExecRequest::new("ls").cwd("/nonexistent/path/xyz");
        let result = validate(&request);
        assert!(matches!(
            result,
            Err(RequestError::InvalidWorkingDirectory(_))
        ));
    }

    #[test]
    fn validate_accepts_none_working_directory() {
        let request = ExecRequest::new("ls");
        assert!(request.cwd.is_none());
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn validate_accepts_valid_environment() {
        let request = ExecRequest::new("ls")
            .env("PATH", "/usr/bin")
            .env("HOME", "/home/user")
            .env("_UNDERSCORE", "value");
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn validate_rejects_empty_environment_key() {
        let mut request = ExecRequest::new("ls");
        request.env.push(("".to_string(), "value".to_string()));
        let result = validate(&request);
        assert!(matches!(result, Err(RequestError::InvalidEnvironment(_))));
    }

    #[test]
    fn validate_rejects_environment_key_starting_with_digit() {
        let mut request = ExecRequest::new("ls");
        request.env.push(("1VAR".to_string(), "value".to_string()));
        let result = validate(&request);
        assert!(matches!(result, Err(RequestError::InvalidEnvironment(_))));
    }

    #[test]
    fn validate_rejects_environment_key_with_invalid_character() {
        let mut request = ExecRequest::new("ls");
        request
            .env
            .push(("VAR-NAME".to_string(), "value".to_string()));
        let result = validate(&request);
        assert!(matches!(result, Err(RequestError::InvalidEnvironment(_))));
    }

    #[test]
    fn validate_rejects_environment_key_with_equals() {
        let mut request = ExecRequest::new("ls");
        request
            .env
            .push(("VAR=NAME".to_string(), "value".to_string()));
        let result = validate(&request);
        assert!(matches!(result, Err(RequestError::InvalidEnvironment(_))));
    }

    #[test]
    fn validate_program_trims_whitespace() {
        let request = ExecRequest::new("  ls  ");
        // Program "  ls  " is not empty after trim check in validation
        // But we check trim().is_empty(), so this should pass
        assert!(validate(&request).is_ok());
    }

    #[test]
    fn validate_working_directory_file_not_directory() {
        // Create a temp file, not directory
        let temp_file = std::env::temp_dir().join("cerberus_test_file");
        std::fs::write(&temp_file, "test").ok();

        let request = ExecRequest::new("ls").cwd(&temp_file);
        let result = validate(&request);

        // Clean up
        std::fs::remove_file(&temp_file).ok();

        assert!(matches!(
            result,
            Err(RequestError::InvalidWorkingDirectory(_))
        ));
    }
}
