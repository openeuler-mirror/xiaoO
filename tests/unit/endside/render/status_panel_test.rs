use super::{MemoryStatus, StatusPanel};

#[test]
fn memory_status_labels_are_operator_readable() {
    assert_eq!(MemoryStatus::Unknown.label(), "UNKNOWN");
    assert_eq!(MemoryStatus::Connected.label(), "OK");
    assert_eq!(MemoryStatus::Degraded.label(), "DEGRADED");
}

#[test]
fn format_context_usage_marks_estimated_values() {
    assert_eq!(StatusPanel::format_context_usage(24_000, true), "~24.0K");
}

#[test]
fn format_context_usage_leaves_reported_values_unmarked() {
    assert_eq!(StatusPanel::format_context_usage(24_000, false), "24.0K");
}
