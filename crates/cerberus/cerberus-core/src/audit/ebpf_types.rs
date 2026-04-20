//! eBPF audit types for kernel-level security event monitoring.
//!
//! These types are used for receiving events from the eBPF audit backend,
//! which monitors security-sensitive operations at the kernel level.
//!
//! # Event Types
//!
//! - [`AuditEvent`]: Top-level enum for all audit events
//! - [`FileAccessEvent`]: File access attempts (Landlock violations)
//! - [`SyscallEvent`]: Syscall attempts (seccomp violations)
//! - [`ForkEvent`]: Process fork events
//! - [`NetworkAccessEvent`]: Network access events

use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::SystemTime;

/// Raw event from eBPF. Must match the Event struct in bpf/src/main.rs exactly.
/// Total size: 280 bytes
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BpfRawEvent {
    /// Event type (FILE_ACCESS, FORK, EXEC, NETWORK)
    pub event_type: u32,
    /// Process ID that triggered the event
    pub pid: u32,
    /// Cgroup ID for container context
    pub cgroup_id: u64,
    /// Event timestamp in nanoseconds since boot
    pub timestamp: u64,
    /// Event-specific data (256 bytes)
    pub data: [u8; 256],
}

// Static assertion for struct size
const _: () = assert!(std::mem::size_of::<BpfRawEvent>() == 280);

// Event type constants (must match BPF definitions)
/// File access event type
pub const EVENT_FILE_ACCESS: u32 = 1;
/// Fork event type
pub const EVENT_FORK: u32 = 2;
/// Exec event type
pub const EVENT_EXEC: u32 = 3;
/// Network event type
pub const EVENT_NETWORK: u32 = 4;

// Data field offsets for FileAccess events
/// File access flags offset in BpfRawEvent.data
pub const FILE_ACCESS_FLAGS_OFFSET: usize = 0;
/// File access mode offset in BpfRawEvent.data
pub const FILE_ACCESS_MODE_OFFSET: usize = 4;
/// File access return value offset in BpfRawEvent.data
pub const FILE_ACCESS_RETVAL_OFFSET: usize = 8;
/// File access path offset in BpfRawEvent.data
pub const FILE_ACCESS_PATH_OFFSET: usize = 16;

// Data field offsets for Fork events
/// Fork parent PID offset in BpfRawEvent.data
pub const FORK_PARENT_PID_OFFSET: usize = 0;
/// Fork child PID offset in BpfRawEvent.data
pub const FORK_CHILD_PID_OFFSET: usize = 4;
/// Fork count offset in BpfRawEvent.data
pub const FORK_COUNT_OFFSET: usize = 8;
/// Fork clone flags offset in BpfRawEvent.data
pub const FORK_CLONE_FLAGS_OFFSET: usize = 16;

// Data field offsets for Exec events
/// Exec filename offset in BpfRawEvent.data
pub const EXEC_FILENAME_OFFSET: usize = 0;

// Data field offsets for Network events
/// Network direction offset in BpfRawEvent.data
pub const NETWORK_DIRECTION_OFFSET: usize = 0;
/// Network protocol offset in BpfRawEvent.data
pub const NETWORK_PROTOCOL_OFFSET: usize = 4;
/// Network address offset in BpfRawEvent.data
pub const NETWORK_ADDRESS_OFFSET: usize = 8;
/// Network port offset in BpfRawEvent.data
pub const NETWORK_PORT_OFFSET: usize = 12;
/// Network result offset in BpfRawEvent.data
pub const NETWORK_RESULT_OFFSET: usize = 14;

/// Network protocol type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkProtocol {
    /// TCP protocol
    Tcp,
    /// UDP protocol
    Udp,
    /// Other protocol (raw protocol number)
    Other(u8),
}

/// Network traffic direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkDirection {
    /// Outbound connection
    Outbound,
    /// Inbound connection
    Inbound,
}

/// Result of network access attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkAccessResult {
    /// Access allowed by policy
    Allowed,
    /// Access denied by policy
    DeniedByPolicy,
    /// Access monitored but not blocked
    Monitored,
}

/// Network access event from eBPF monitoring.
#[derive(Debug, Clone)]
pub struct NetworkAccessEvent {
    /// Traffic direction
    pub direction: NetworkDirection,
    /// Network protocol
    pub protocol: NetworkProtocol,
    /// Remote address
    pub address: Ipv4Addr,
    /// Remote port
    pub port: u16,
    /// Access result
    pub result: NetworkAccessResult,
    /// Process ID
    pub pid: u32,
    /// Event timestamp
    pub timestamp: SystemTime,
}

/// File access event from Landlock monitoring.
#[derive(Debug, Clone)]
pub struct FileAccessEvent {
    /// Path being accessed
    pub path: PathBuf,
    /// Access operation type
    pub operation: FileOperation,
    /// Access result
    pub result: FileAccessResult,
    /// Process ID
    pub pid: u32,
    /// Event timestamp
    pub timestamp: SystemTime,
}

/// File operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    /// Read operation
    Read,
    /// Write operation
    Write,
    /// Execute operation
    Execute,
}

/// Result of file access attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAccessResult {
    /// Access allowed
    Allowed,
    /// Access denied by Landlock LSM
    DeniedByLandlock,
    /// Access denied due to path not found
    DeniedByPathNotFound,
}

/// Syscall event from seccomp monitoring.
#[derive(Debug, Clone)]
pub struct SyscallEvent {
    /// Syscall name
    pub name: String,
    /// Syscall number
    pub syscall_nr: i64,
    /// Syscall arguments
    pub args: [u64; 6],
    /// Syscall result
    pub result: SyscallResult,
    /// Process ID
    pub pid: u32,
    /// Event timestamp
    pub timestamp: SystemTime,
}

/// Result of syscall attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallResult {
    /// Syscall allowed
    Allowed,
    /// Syscall denied by seccomp
    DeniedBySeccomp,
}

/// Process fork event.
#[derive(Debug, Clone)]
pub struct ForkEvent {
    /// Parent process ID
    pub parent_pid: u32,
    /// Child process ID
    pub child_pid: u32,
    /// Total fork count
    pub fork_count: u64,
    /// Fork limit if configured
    pub fork_limit: Option<u64>,
    /// Event timestamp
    pub timestamp: SystemTime,
}

/// Top-level audit event from eBPF monitoring.
#[derive(Debug, Clone)]
pub enum EbpfAuditEvent {
    /// File access event
    FileAccess(FileAccessEvent),
    /// Syscall event
    Syscall(SyscallEvent),
    /// Fork event
    Fork(ForkEvent),
    /// Network access event
    Network(NetworkAccessEvent),
}

impl EbpfAuditEvent {
    /// Get the timestamp of this event.
    pub fn timestamp(&self) -> SystemTime {
        match self {
            EbpfAuditEvent::FileAccess(e) => e.timestamp,
            EbpfAuditEvent::Syscall(e) => e.timestamp,
            EbpfAuditEvent::Fork(e) => e.timestamp,
            EbpfAuditEvent::Network(e) => e.timestamp,
        }
    }

    /// Get the event type name.
    pub fn event_type(&self) -> &'static str {
        match self {
            EbpfAuditEvent::FileAccess(_) => "FILE_ACCESS",
            EbpfAuditEvent::Syscall(_) => "SYSCALL",
            EbpfAuditEvent::Fork(_) => "FORK",
            EbpfAuditEvent::Network(_) => "NETWORK",
        }
    }
}

impl std::fmt::Display for EbpfAuditEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EbpfAuditEvent::FileAccess(e) => {
                write!(
                    f,
                    "[FILE_ACCESS] {} {:?} -> {:?}",
                    e.path.display(),
                    e.operation,
                    e.result
                )
            }
            EbpfAuditEvent::Syscall(e) => {
                write!(
                    f,
                    "[SYSCALL] {} ({}) -> {:?}",
                    e.name, e.syscall_nr, e.result
                )
            }
            EbpfAuditEvent::Fork(e) => {
                write!(
                    f,
                    "[FORK] {} -> {} (count: {})",
                    e.parent_pid, e.child_pid, e.fork_count
                )
            }
            EbpfAuditEvent::Network(e) => {
                write!(
                    f,
                    "[NETWORK] pid {} {} {} {}:{} -> {}",
                    e.pid, e.direction, e.protocol, e.address, e.port, e.result
                )
            }
        }
    }
}

impl std::fmt::Display for NetworkProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkProtocol::Tcp => write!(f, "TCP"),
            NetworkProtocol::Udp => write!(f, "UDP"),
            NetworkProtocol::Other(n) => write!(f, "OTHER({})", n),
        }
    }
}

impl std::fmt::Display for NetworkDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkDirection::Outbound => write!(f, "OUTBOUND"),
            NetworkDirection::Inbound => write!(f, "INBOUND"),
        }
    }
}

impl std::fmt::Display for NetworkAccessResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkAccessResult::Allowed => write!(f, "ALLOWED"),
            NetworkAccessResult::DeniedByPolicy => write!(f, "DENIED_BY_POLICY"),
            NetworkAccessResult::Monitored => write!(f, "MONITORED"),
        }
    }
}

impl std::fmt::Display for NetworkAccessEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NetworkAccessEvent {{ direction: {}, protocol: {}, address: {}, port: {}, result: {}, pid: {}, timestamp: {:?} }}",
            self.direction, self.protocol, self.address, self.port, self.result, self.pid, self.timestamp
        )
    }
}
