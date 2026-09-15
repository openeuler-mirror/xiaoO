use crate::gateway::MemoryAutomationHealth;
use crate::status_panel::MemoryStatus;

use super::memory_status_from_health;

#[test]
fn memory_health_maps_to_operator_status() {
    assert_eq!(
        memory_status_from_health(MemoryAutomationHealth::Healthy),
        MemoryStatus::Connected
    );
    assert_eq!(
        memory_status_from_health(MemoryAutomationHealth::Degraded),
        MemoryStatus::Degraded
    );
}
