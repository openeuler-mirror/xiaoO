//! Serializable network access audit records collected during execution.

use super::ebpf_types::system_time_to_nanos;
use super::{NetworkAccessEvent, NetworkAccessResult, NetworkDirection, NetworkProtocol};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// A single network access event collected during one Cerberus execution.
///
/// `result` carries the *enforcement* outcome (what the policy decided for this
/// connection), not the raw kernel-reported status, so the record reflects the
/// authoritative allow/deny/monitor decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkAccessRecord {
    pub direction: NetworkAccessDirection,
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub result: NetworkAccessOutcome,
    pub pid: u32,
    pub timestamp_nanos: u64,
}

/// Traffic direction recorded in audit output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccessDirection {
    Outbound,
    Inbound,
}

/// Enforcement outcome of a network access attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAccessOutcome {
    Allowed,
    DeniedByPolicy,
    Monitored,
}

impl NetworkAccessRecord {
    /// Build a record from a kernel event plus the policy enforcement result.
    pub fn from_event(event: &NetworkAccessEvent, result: NetworkAccessResult) -> Self {
        Self {
            direction: event.direction.into(),
            protocol: protocol_label(&event.protocol),
            address: event.address.to_string(),
            port: event.port,
            result: result.into(),
            pid: event.pid,
            timestamp_nanos: system_time_to_nanos(event.timestamp),
        }
    }
}

/// Lowercase, JSON-friendly label for a protocol (mirrors the snake_case style
/// of the enum fields without losing the raw number of unknown protocols).
fn protocol_label(protocol: &NetworkProtocol) -> String {
    match protocol {
        NetworkProtocol::Tcp => "tcp".to_string(),
        NetworkProtocol::Udp => "udp".to_string(),
        NetworkProtocol::Other(number) => format!("other({})", number),
    }
}

impl From<NetworkDirection> for NetworkAccessDirection {
    fn from(direction: NetworkDirection) -> Self {
        match direction {
            NetworkDirection::Outbound => Self::Outbound,
            NetworkDirection::Inbound => Self::Inbound,
        }
    }
}

impl From<NetworkAccessResult> for NetworkAccessOutcome {
    fn from(result: NetworkAccessResult) -> Self {
        match result {
            NetworkAccessResult::Allowed => Self::Allowed,
            NetworkAccessResult::DeniedByPolicy => Self::DeniedByPolicy,
            NetworkAccessResult::Monitored => Self::Monitored,
        }
    }
}

/// In-memory collector for network access events during one execution.
#[derive(Debug, Default)]
pub struct NetworkAccessCollector {
    events: Mutex<Vec<NetworkAccessRecord>>,
}

impl NetworkAccessCollector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn record(&self, event: &NetworkAccessEvent, result: NetworkAccessResult) {
        self.events
            .lock()
            .expect("network access collector mutex poisoned")
            .push(NetworkAccessRecord::from_event(event, result));
    }

    pub fn take(&self) -> Vec<NetworkAccessRecord> {
        std::mem::take(
            &mut *self
                .events
                .lock()
                .expect("network access collector mutex poisoned"),
        )
    }

    /// Number of records collected so far (without draining).
    pub fn len(&self) -> usize {
        self.events
            .lock()
            .expect("network access collector mutex poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{NetworkDirection, NetworkProtocol};
    use std::net::Ipv4Addr;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn sample_event(result: NetworkAccessResult) -> NetworkAccessEvent {
        NetworkAccessEvent {
            direction: NetworkDirection::Outbound,
            protocol: NetworkProtocol::Tcp,
            address: Ipv4Addr::new(8, 8, 8, 8),
            port: 443,
            result,
            pid: 4242,
            timestamp: UNIX_EPOCH + Duration::from_secs(1),
        }
    }

    #[test]
    fn record_uses_enforcement_result_not_event_result() {
        // Kernel event says Allowed, but enforcement decided DeniedByPolicy.
        let event = NetworkAccessEvent {
            direction: NetworkDirection::Outbound,
            protocol: NetworkProtocol::Tcp,
            address: Ipv4Addr::new(1, 1, 1, 1),
            port: 80,
            result: NetworkAccessResult::Allowed,
            pid: 7,
            timestamp: UNIX_EPOCH + Duration::from_secs(1),
        };

        let record = NetworkAccessRecord::from_event(&event, NetworkAccessResult::DeniedByPolicy);
        assert_eq!(record.direction, NetworkAccessDirection::Outbound);
        assert_eq!(record.protocol, "tcp");
        assert_eq!(record.address, "1.1.1.1");
        assert_eq!(record.port, 80);
        assert_eq!(record.result, NetworkAccessOutcome::DeniedByPolicy);
        assert_eq!(record.pid, 7);
        assert_eq!(record.timestamp_nanos, 1_000_000_000);
    }

    #[test]
    fn protocol_other_keeps_raw_number() {
        let event = NetworkAccessEvent {
            protocol: NetworkProtocol::Other(132),
            ..sample_event(NetworkAccessResult::Allowed)
        };
        let record = NetworkAccessRecord::from_event(&event, NetworkAccessResult::Allowed);
        assert_eq!(record.protocol, "other(132)");
    }

    #[test]
    fn collector_records_and_takes_events() {
        let collector = NetworkAccessCollector::new();
        let event = sample_event(NetworkAccessResult::Monitored);
        collector.record(&event, NetworkAccessResult::Monitored);

        let records = collector.take();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].result, NetworkAccessOutcome::Monitored);
        assert!(collector.take().is_empty());
    }
}
