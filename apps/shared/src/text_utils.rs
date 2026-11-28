//! Shared string utilities for config/env values.

/// Trimmed value when not blank, `None` otherwise: the shared
/// "blank-after-trim counts as unset" rule.
pub fn non_blank_trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
