//! Cron expression parsing and scheduling.
//!
//! Wraps the `cron` crate to provide validated cron expressions with
//! support for both standard 5-field and extended 6-field (seconds) formats.

use std::str::FromStr;

/// A validated cron expression with cached schedule.
///
/// Supports standard 5-field (`min hour dom month dow`) and
/// 6-field (`sec min hour dom month dow`) formats.
///
/// The parsed `Schedule` is cached to avoid repeated parsing on each trigger calculation.
#[derive(Debug, Clone)]
pub struct CronExpression {
    /// Original expression string (normalized to 6-field format)
    raw: String,
    /// Cached parsed schedule for efficient iteration
    schedule: cron::Schedule,
}

impl CronExpression {
    /// Parse a cron expression string.
    ///
    /// Accepts both 5-field and 6-field formats.
    /// Returns [`CronParseError`] if the expression is empty or invalid.
    pub fn parse(raw: &str) -> Result<Self, CronParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CronParseError::Empty);
        }

        // The `cron` crate expects: sec min hour dom month dow [year]
        // Standard 5-field cron is:  min hour dom month dow
        // Standard 6-field cron is:  sec min hour dom month dow
        //
        // Normalize by counting fields:
        //   5 fields → prepend "0 " (seconds)
        //   6 fields → use as-is
        let normalized = {
            let field_count = trimmed.split_whitespace().count();
            match field_count {
                5 => format!("0 {trimmed}"),
                6 => trimmed.to_string(),
                n => {
                    return Err(CronParseError::InvalidSyntax {
                        raw: trimmed.to_string(),
                        message: format!(
                            "expected 5 or 6 fields, got {n}. \
                             Use 'min hour dom month dow' (5) \
                             or 'sec min hour dom month dow' (6)"
                        ),
                    });
                }
            }
        };

        // Let the cron crate validate the normalized expression and cache the result
        let schedule =
            cron::Schedule::from_str(&normalized).map_err(|e| CronParseError::InvalidSyntax {
                raw: trimmed.to_string(),
                message: e.to_string(),
            })?;

        Ok(Self {
            raw: normalized,
            schedule,
        })
    }

    /// Compute the next trigger time strictly after `after`.
    ///
    /// Returns `None` if the expression will never fire again
    /// (extremely rare, e.g. an expression limited to a past year).
    pub fn next_after(
        &self,
        after: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        // Use cached schedule instead of re-parsing
        self.schedule.after(&after).next()
    }

    /// Return the normalized cron string (6-field format).
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl std::fmt::Display for CronExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.raw.fmt(f)
    }
}

impl PartialEq for CronExpression {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl Eq for CronExpression {}

/// Errors returned by [`CronExpression::parse`].
#[derive(Debug, thiserror::Error)]
pub enum CronParseError {
    #[error("empty cron expression")]
    Empty,

    #[error("invalid cron expression '{raw}': {message}")]
    InvalidSyntax { raw: String, message: String },
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-types/cron/expression_test.rs"]
mod tests;
