/// Validation-related constants for FileWriteTool.

/// Error codes for validation failures.
pub mod error_code {
    /// Secret detected in content (error_code = 0)
    pub const SECRET_DETECTED: u32 = 0;
}

/// Error message returned when secret-like content is detected.
pub const SECRET_DETECTED_MESSAGE: &str = "Secret detected in content";

/// Secret patterns that should trigger secret detection.
pub const SECRET_PATTERNS: &[&str] = &[
    "password",
    "secret",
    "token",
    "api_key",
    "apikey",
    "api-key",
    "private_key",
    "privatekey",
    "access_token",
    "auth_token",
];
