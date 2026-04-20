//! eBPF program loader and event parser.

use aya::maps::perf::PerfEventArray;
use aya::programs::{ProgramError, TracePoint};
use aya::util::online_cpus;
use aya::{include_bytes_aligned, Ebpf};
use bytes::BytesMut;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use crate::audit::{
    BpfRawEvent, EbpfAuditEvent, FileAccessEvent, FileAccessResult, FileOperation, ForkEvent,
    NetworkAccessEvent, NetworkAccessResult, NetworkDirection, NetworkProtocol, SyscallEvent,
    SyscallResult, EVENT_EXEC, EVENT_FILE_ACCESS, EVENT_FORK, EVENT_NETWORK, EXEC_FILENAME_OFFSET,
    FILE_ACCESS_FLAGS_OFFSET, FILE_ACCESS_PATH_OFFSET, FILE_ACCESS_RETVAL_OFFSET,
    FORK_CHILD_PID_OFFSET, FORK_COUNT_OFFSET, FORK_PARENT_PID_OFFSET,
};

/// Tracepoints to attach for monitoring.
const TRACEPOINTS: [(&str, &str, &str); 20] = [
    ("syscalls", "sys_enter_openat", "syscalls_sys_enter_openat"),
    ("syscalls", "sys_exit_openat", "syscalls_sys_exit_openat"),
    (
        "syscalls",
        "sys_enter_openat2",
        "syscalls_sys_enter_openat2",
    ),
    ("syscalls", "sys_exit_openat2", "syscalls_sys_exit_openat2"),
    ("syscalls", "sys_enter_clone", "syscalls_sys_enter_clone"),
    ("syscalls", "sys_exit_clone", "syscalls_sys_exit_clone"),
    ("syscalls", "sys_enter_clone3", "syscalls_sys_enter_clone3"),
    ("syscalls", "sys_exit_clone3", "syscalls_sys_exit_clone3"),
    ("syscalls", "sys_enter_fork", "syscalls_sys_enter_fork"),
    ("syscalls", "sys_exit_fork", "syscalls_sys_exit_fork"),
    ("syscalls", "sys_enter_vfork", "syscalls_sys_enter_vfork"),
    ("syscalls", "sys_exit_vfork", "syscalls_sys_exit_vfork"),
    ("syscalls", "sys_enter_execve", "syscalls_sys_enter_execve"),
    (
        "syscalls",
        "sys_enter_execveat",
        "syscalls_sys_enter_execveat",
    ),
    ("syscalls", "sys_enter_socket", "syscalls_sys_enter_socket"),
    ("syscalls", "sys_exit_socket", "syscalls_sys_exit_socket"),
    (
        "syscalls",
        "sys_enter_connect",
        "syscalls_sys_enter_connect",
    ),
    ("syscalls", "sys_exit_connect", "syscalls_sys_exit_connect"),
    ("syscalls", "sys_enter_bind", "syscalls_sys_enter_bind"),
    ("syscalls", "sys_exit_bind", "syscalls_sys_exit_bind"),
];

/// Configuration for the eBPF program.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Config {
    /// Target cgroup ID to monitor (0 for all).
    pub target_cgroup_id: u64,
    /// Maximum number of forks allowed.
    pub max_forks: u64,
    /// Whether monitoring is enabled.
    pub enabled: u32,
    /// Padding for alignment.
    pub _padding: u32,
}

unsafe impl aya::Pod for Config {}

/// eBPF program loader.
pub struct EbpfLoader {
    bpf: Ebpf,
}

impl EbpfLoader {
    /// Load the eBPF program from the pre-compiled object file.
    pub fn load() -> Result<Self, EbpfLoadError> {
        // The eBPF object file is built by the build script from the BPF program
        // in the `bpf/` directory. For now, we use a placeholder that will be
        // replaced by the actual build output.
        let bpf_bytes = include_bytes_aligned!(concat!(env!("OUT_DIR"), "/audit"));
        let bpf = Ebpf::load(bpf_bytes)?;
        Ok(Self { bpf })
    }

    /// Set configuration after programs are attached.
    ///
    /// This must be called after `attach()` because `take_map()` removes
    /// the map from the BPF object.
    pub fn set_config(
        &mut self,
        target_cgroup_id: u64,
        max_forks: u64,
        enabled: bool,
    ) -> Result<(), EbpfLoadError> {
        use aya::maps::HashMap;

        let mut config_map: HashMap<_, u32, Config> = HashMap::try_from(
            self.bpf
                .take_map("CONFIG")
                .ok_or_else(|| EbpfLoadError::MapNotFound("CONFIG".to_string()))?,
        )
        .map_err(|e| EbpfLoadError::MapError(e.to_string()))?;

        let config = Config {
            target_cgroup_id,
            max_forks,
            enabled: if enabled { 1 } else { 0 },
            _padding: 0,
        };

        config_map
            .insert(0, config, 0)
            .map_err(|e| EbpfLoadError::MapError(e.to_string()))?;

        log::info!(
            "Set config: cgroup={}, max_forks={}, enabled={}",
            target_cgroup_id,
            max_forks,
            enabled
        );
        Ok(())
    }

    /// Attach all tracepoint programs.
    pub fn attach(&mut self) -> Result<(), EbpfLoadError> {
        let mut attached_count = 0;
        let mut failed_tracepoints = Vec::new();

        for (category, name, program_name) in TRACEPOINTS {
            match self.attach_single_tracepoint(category, name, program_name) {
                Ok(()) => attached_count += 1,
                Err(e) => {
                    log::warn!("Failed to attach tracepoint {}:{}: {}", category, name, e);
                    failed_tracepoints.push(format!("{}:{}", category, name));
                }
            }
        }

        if attached_count == 0 {
            return Err(EbpfLoadError::AttachError(format!(
                "Failed to attach any tracepoints. All {} attempts failed.",
                TRACEPOINTS.len()
            )));
        }

        log::info!(
            "Attached {}/{} tracepoints successfully",
            attached_count,
            TRACEPOINTS.len()
        );
        if !failed_tracepoints.is_empty() {
            log::warn!(
                "Skipped unavailable tracepoints: {}",
                failed_tracepoints.join(", ")
            );
        }

        Ok(())
    }

    fn attach_single_tracepoint(
        &mut self,
        category: &str,
        name: &str,
        program_name: &str,
    ) -> Result<(), EbpfLoadError> {
        let program: &mut TracePoint = self
            .bpf
            .program_mut(program_name)
            .ok_or_else(|| EbpfLoadError::ProgramNotFound(program_name.to_string()))?
            .try_into()
            .map_err(|e: ProgramError| EbpfLoadError::AttachError(e.to_string()))?;

        program
            .load()
            .map_err(|e| EbpfLoadError::AttachError(e.to_string()))?;
        program
            .attach(category, name)
            .map_err(|e| EbpfLoadError::AttachError(e.to_string()))?;

        log::info!(
            "Attached tracepoint: {}:{} -> {}",
            category,
            name,
            program_name
        );
        Ok(())
    }

    /// Open perf buffers for receiving events from the kernel.
    pub fn open_perf_buffers(
        &mut self,
        buffer_size: usize,
    ) -> Result<mpsc::Receiver<EbpfAuditEvent>, EbpfLoadError> {
        let events_map = self
            .bpf
            .take_map("EVENTS")
            .ok_or_else(|| EbpfLoadError::MapNotFound("EVENTS".to_string()))?;

        let mut perf_array: PerfEventArray<aya::maps::MapData> =
            PerfEventArray::try_from(events_map)
                .map_err(|e| EbpfLoadError::PerfEventArrayError(e.to_string()))?;

        let (event_tx, event_rx) = mpsc::channel(buffer_size);

        let cpus = online_cpus().map_err(|e| {
            EbpfLoadError::PerfEventArrayError(format!("Failed to get online CPUs: {}", e.1))
        })?;

        for cpu_id in cpus {
            let buf = perf_array.open(cpu_id, None).map_err(|e| {
                EbpfLoadError::PerfEventArrayError(format!(
                    "Failed to open perf buffer for CPU {}: {:?}",
                    cpu_id, e
                ))
            })?;

            let tx = event_tx.clone();
            tokio::spawn(async move {
                Self::read_perf_events_loop(buf, tx).await;
            });
        }

        Ok(event_rx)
    }

    async fn read_perf_events_loop(
        buf: aya::maps::perf::PerfEventArrayBuffer<aya::maps::MapData>,
        event_tx: mpsc::Sender<EbpfAuditEvent>,
    ) {
        let mut async_buf =
            match tokio::io::unix::AsyncFd::with_interest(buf, tokio::io::Interest::READABLE) {
                Ok(b) => b,
                Err(e) => {
                    log::error!("Failed to create async perf buffer: {:?}", e);
                    return;
                }
            };

        let mut buffers: Vec<BytesMut> = std::iter::repeat_with(|| BytesMut::with_capacity(4096))
            .take(10)
            .collect();

        loop {
            let mut guard = match async_buf.readable_mut().await {
                Ok(g) => g,
                Err(_) => {
                    log::warn!("Perf buffer closed, stopping event loop");
                    return;
                }
            };

            loop {
                let events = match guard.get_inner_mut().read_events(&mut buffers) {
                    Ok(e) => e,
                    Err(e) => {
                        log::error!("Failed to read perf events: {:?}", e);
                        break;
                    }
                };

                for buf in buffers.iter_mut().take(events.read) {
                    let raw_event = Self::parse_raw_event(buf);
                    if let Some(event) = Self::convert_event(raw_event) {
                        if event_tx.send(event).await.is_err() {
                            log::warn!("Event channel closed, stopping event loop");
                            return;
                        }
                    }
                }

                if events.read != buffers.len() {
                    break;
                }
            }

            guard.clear_ready();
        }
    }

    /// Parse raw bytes into a BpfRawEvent.
    pub(crate) fn parse_raw_event(data: &[u8]) -> BpfRawEvent {
        if data.len() < 24 {
            return default_bpf_raw_event();
        }

        BpfRawEvent {
            event_type: u32::from_ne_bytes([data[0], data[1], data[2], data[3]]),
            pid: u32::from_ne_bytes([data[4], data[5], data[6], data[7]]),
            cgroup_id: u64::from_ne_bytes([
                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
            ]),
            timestamp: u64::from_ne_bytes([
                data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            ]),
            data: {
                let mut arr = [0u8; 256];
                let copy_len = std::cmp::min(256, data.len().saturating_sub(24));
                if copy_len > 0 {
                    arr[..copy_len].copy_from_slice(&data[24..24 + copy_len]);
                }
                arr
            },
        }
    }

    fn convert_event(raw: BpfRawEvent) -> Option<EbpfAuditEvent> {
        let timestamp = UNIX_EPOCH + Duration::from_nanos(raw.timestamp);

        match raw.event_type {
            EVENT_FILE_ACCESS => Self::parse_file_access_event(&raw, timestamp),
            EVENT_FORK => Self::parse_fork_event(&raw, timestamp),
            EVENT_EXEC => Self::parse_exec_event(&raw, timestamp),
            EVENT_NETWORK => Self::parse_network_event(&raw, timestamp),
            _ => {
                log::warn!("Unknown event type: {}", raw.event_type);
                None
            }
        }
    }

    fn parse_file_access_event(raw: &BpfRawEvent, timestamp: SystemTime) -> Option<EbpfAuditEvent> {
        const ENOENT: i64 = -2;
        const EACCES: i64 = -13;

        let flags = read_u32_at(&raw.data, FILE_ACCESS_FLAGS_OFFSET);
        let retval = read_i64_at(&raw.data, FILE_ACCESS_RETVAL_OFFSET);

        let path = extract_null_terminated_string(&raw.data, FILE_ACCESS_PATH_OFFSET);

        let file_op = derive_file_operation(flags);
        let file_result = if retval >= 0 {
            FileAccessResult::Allowed
        } else if retval == ENOENT {
            FileAccessResult::DeniedByPathNotFound
        } else if retval == EACCES {
            FileAccessResult::DeniedByLandlock
        } else {
            FileAccessResult::DeniedByLandlock
        };

        Some(EbpfAuditEvent::FileAccess(FileAccessEvent {
            path: PathBuf::from(path),
            operation: file_op,
            result: file_result,
            pid: raw.pid,
            timestamp,
        }))
    }

    fn parse_fork_event(raw: &BpfRawEvent, timestamp: SystemTime) -> Option<EbpfAuditEvent> {
        let parent_pid = read_u32_at(&raw.data, FORK_PARENT_PID_OFFSET);
        let child_pid = read_u32_at(&raw.data, FORK_CHILD_PID_OFFSET);
        let fork_count = read_u64_at(&raw.data, FORK_COUNT_OFFSET);

        Some(EbpfAuditEvent::Fork(ForkEvent {
            parent_pid,
            child_pid,
            fork_count,
            fork_limit: None,
            timestamp,
        }))
    }

    fn parse_exec_event(raw: &BpfRawEvent, timestamp: SystemTime) -> Option<EbpfAuditEvent> {
        let filename = extract_null_terminated_string(&raw.data, EXEC_FILENAME_OFFSET);

        Some(EbpfAuditEvent::Syscall(SyscallEvent {
            name: filename,
            syscall_nr: 59,
            args: [0u64; 6],
            result: SyscallResult::Allowed,
            pid: raw.pid,
            timestamp,
        }))
    }

    fn parse_network_event(raw: &BpfRawEvent, timestamp: SystemTime) -> Option<EbpfAuditEvent> {
        let data = &raw.data;

        // Parse direction (offset 0, u32)
        let direction_val = u32::from_ne_bytes([data[0], data[1], data[2], data[3]]);
        let direction = if direction_val == 0 {
            NetworkDirection::Outbound
        } else {
            NetworkDirection::Inbound
        };

        // Parse protocol (offset 4, u32)
        let protocol_val = u32::from_ne_bytes([data[4], data[5], data[6], data[7]]);
        let protocol = match protocol_val {
            1 => NetworkProtocol::Tcp, // SOCK_STREAM
            2 => NetworkProtocol::Udp, // SOCK_DGRAM
            n => NetworkProtocol::Other(n as u8),
        };

        // Parse IP address (offset 8, u32 in network byte order)
        let addr_bytes = [data[8], data[9], data[10], data[11]];
        let address = Ipv4Addr::from(addr_bytes);

        // Parse port (offset 12, u16 in network byte order)
        let port = u16::from_be_bytes([data[12], data[13]]);

        // Result is always Allowed at parsing time (enforcement happens in user-space later)
        let result = NetworkAccessResult::Allowed;

        Some(EbpfAuditEvent::Network(NetworkAccessEvent {
            direction,
            protocol,
            address,
            port,
            result,
            pid: raw.pid,
            timestamp,
        }))
    }

    /// Take ownership of the underlying BPF object.
    pub fn take_bpf(self) -> Ebpf {
        self.bpf
    }
}

fn read_u32_at(data: &[u8], offset: usize) -> u32 {
    if offset + 4 > data.len() {
        return 0;
    }
    u32::from_ne_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_i64_at(data: &[u8], offset: usize) -> i64 {
    if offset + 8 > data.len() {
        return 0;
    }
    i64::from_ne_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

fn read_u64_at(data: &[u8], offset: usize) -> u64 {
    if offset + 8 > data.len() {
        return 0;
    }
    u64::from_ne_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

fn extract_null_terminated_string(data: &[u8], offset: usize) -> String {
    let end = data[offset..]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(data.len() - offset);
    String::from_utf8_lossy(&data[offset..offset + end]).into_owned()
}

fn derive_file_operation(flags: u32) -> FileOperation {
    const O_ACCMODE: u32 = 3;
    const O_WRONLY: u32 = 1;
    const O_RDWR: u32 = 2;
    const O_CREAT: u32 = 0o100;
    const O_TRUNC: u32 = 0o1000;
    const O_EXEC: u32 = 0o100000;

    if flags & O_EXEC != 0 {
        return FileOperation::Execute;
    }

    match flags & O_ACCMODE {
        O_WRONLY | O_RDWR => FileOperation::Write,
        _ if flags & O_CREAT != 0 || flags & O_TRUNC != 0 => FileOperation::Write,
        _ => FileOperation::Read,
    }
}

fn default_bpf_raw_event() -> BpfRawEvent {
    BpfRawEvent {
        event_type: 0,
        pid: 0,
        cgroup_id: 0,
        timestamp: 0,
        data: [0u8; 256],
    }
}

/// Errors that can occur when loading eBPF programs.
#[derive(Debug, thiserror::Error)]
pub enum EbpfLoadError {
    /// Failed to load the eBPF program.
    #[error("Failed to load eBPF program: {0}")]
    LoadError(#[from] aya::EbpfError),

    /// Failed to attach the eBPF program.
    #[error("Failed to attach eBPF program: {0}")]
    AttachError(String),

    /// eBPF is not supported on this platform.
    #[error("eBPF not supported: {0}")]
    NotSupported(String),

    /// The specified program was not found.
    #[error("Program not found: {0}")]
    ProgramNotFound(String),

    /// The specified map was not found.
    #[error("Map not found: {0}")]
    MapNotFound(String),

    /// An error occurred while accessing a map.
    #[error("Map error: {0}")]
    MapError(String),

    /// An error occurred while setting up perf event arrays.
    #[error("PerfEventArray error: {0}")]
    PerfEventArrayError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ebpf_load_error_display() {
        let err = EbpfLoadError::ProgramNotFound("trace_openat".to_string());
        assert!(format!("{}", err).contains("Program not found"));
    }

    #[test]
    fn test_bpf_raw_event_default() {
        let raw = default_bpf_raw_event();
        assert_eq!(raw.event_type, 0);
        assert_eq!(raw.pid, 0);
        assert_eq!(raw.data.len(), 256);
    }

    #[test]
    fn test_parse_raw_event() {
        let mut data = vec![0u8; 280];
        data[0..4].copy_from_slice(&2u32.to_ne_bytes());
        data[4..8].copy_from_slice(&999u32.to_ne_bytes());
        data[8..16].copy_from_slice(&123u64.to_ne_bytes());
        data[16..24].copy_from_slice(&456789u64.to_ne_bytes());
        data[24..28].copy_from_slice(&[1, 2, 3, 4]);

        let raw = EbpfLoader::parse_raw_event(&data);

        assert_eq!(raw.event_type, 2);
        assert_eq!(raw.pid, 999);
        assert_eq!(raw.cgroup_id, 123);
        assert_eq!(raw.timestamp, 456789);
    }

    #[test]
    fn test_derive_file_operation() {
        assert_eq!(derive_file_operation(0), FileOperation::Read);
        assert_eq!(derive_file_operation(1), FileOperation::Write);
        assert_eq!(derive_file_operation(2), FileOperation::Write);
        assert_eq!(derive_file_operation(0o100000), FileOperation::Execute);
    }

    #[test]
    fn test_tracepoints_array_size() {
        assert_eq!(TRACEPOINTS.len(), 20, "Should have 20 tracepoints defined");
    }
}
