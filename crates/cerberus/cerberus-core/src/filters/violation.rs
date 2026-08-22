//! Violation actions for filter rules.
//!
//! Defines what action to take when a filter rule is violated.

use serde::{Deserialize, Serialize};

/// Action to take when a filter violation is detected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViolationAction {
    /// Log a warning but allow execution to continue.
    ///
    /// Use for non-critical violations that should be monitored
    /// but don't pose an immediate security risk.
    #[default]
    Warn,

    /// Reject the request entirely.
    ///
    /// The execution is blocked before it starts.
    /// Use for violations that would compromise security.
    Reject,

    /// Terminate the running process.
    ///
    /// Use for violations detected during execution that
    /// require immediate termination (e.g., output containing secrets).
    Terminate,
}

impl std::fmt::Display for ViolationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Warn => write!(f, "warn"),
            Self::Reject => write!(f, "reject"),
            Self::Terminate => write!(f, "terminate"),
        }
    }
}

/// Result of a violation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationResult {
    /// Whether a violation was detected.
    pub violated: bool,
    /// The action to take if violated.
    pub action: ViolationAction,
    /// Human-readable description of the violation.
    pub reason: String,
}

impl ViolationResult {
    /// Create a no-violation result.
    pub fn ok() -> Self {
        Self {
            violated: false,
            action: ViolationAction::Warn,
            reason: String::new(),
        }
    }

    /// Create a violation result.
    pub fn violation(action: ViolationAction, reason: impl Into<String>) -> Self {
        Self {
            violated: true,
            action,
            reason: reason.into(),
        }
    }

    /// Check if execution should be blocked.
    pub fn should_reject(&self) -> bool {
        self.violated && self.action == ViolationAction::Reject
    }

    /// Check if process should be terminated.
    pub fn should_terminate(&self) -> bool {
        self.violated && self.action == ViolationAction::Terminate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violation_action_default_is_warn() {
        assert_eq!(ViolationAction::default(), ViolationAction::Warn);
    }

    #[test]
    fn violation_action_display() {
        assert_eq!(format!("{}", ViolationAction::Warn), "warn");
        assert_eq!(format!("{}", ViolationAction::Reject), "reject");
        assert_eq!(format!("{}", ViolationAction::Terminate), "terminate");
    }

    #[test]
    fn violation_action_serde() {
        let action = ViolationAction::Reject;
        let json = serde_json::to_string(&action).unwrap();
        assert_eq!(json, "\"reject\"");

        let parsed: ViolationAction = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, action);
    }

    #[test]
    fn violation_result_ok() {
        let result = ViolationResult::ok();
        assert!(!result.violated);
        assert!(!result.should_reject());
        assert!(!result.should_terminate());
    }

    #[test]
    fn violation_result_violation_reject() {
        let result = ViolationResult::violation(ViolationAction::Reject, "dangerous pattern");
        assert!(result.violated);
        assert!(result.should_reject());
        assert!(!result.should_terminate());
        assert_eq!(result.reason, "dangerous pattern");
    }

    #[test]
    fn violation_result_violation_terminate() {
        let result = ViolationResult::violation(ViolationAction::Terminate, "secret detected");
        assert!(result.violated);
        assert!(!result.should_reject());
        assert!(result.should_terminate());
    }

    #[test]
    fn violation_result_violation_warn() {
        let result = ViolationResult::violation(ViolationAction::Warn, "suspicious pattern");
        assert!(result.violated);
        assert!(!result.should_reject());
        assert!(!result.should_terminate());
    }
}
