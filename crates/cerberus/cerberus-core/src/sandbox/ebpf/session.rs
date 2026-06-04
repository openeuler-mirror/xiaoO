//! Unified eBPF audit session for network enforcement and file access collection.

use crate::audit::{EbpfAuditEvent, FileAccessCollector, FileAccessRecord};
use crate::network::{NetworkEnforcer, NetworkPolicyMatcher};
use crate::policy::NetworkPolicy;
use bytes::BytesMut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// Idle sleep between poll sweeps when no events were drained.
const POLL_IDLE_MS: u64 = 1;
/// Per-CPU perf ring buffer size in pages (power of two). 64 pages = 256 KiB,
/// large enough to avoid drops for typical command bursts.
const PERF_BUFFER_PAGES: usize = 64;
/// Number of scratch buffers used per `read_events` call.
const READ_BATCH: usize = 64;
/// Poll interval while draining the perf ring buffers on shutdown.
const DRAIN_POLL_MS: u64 = 10;
/// Minimum grace before the drain can be considered complete, so the reader has
/// time to pull the final events out of the ring buffers.
const DRAIN_MIN_WAIT_MS: u64 = 60;
/// Consecutive stable polls (no new events) that signal the drain is complete.
const DRAIN_STABLE_ROUNDS: u32 = 3;
/// Upper bound on how long finish() waits for the ring buffers to drain.
const DRAIN_MAX_WAIT_MS: u64 = 2000;

/// Configuration for a per-execution eBPF audit session.
#[derive(Debug, Clone, Default)]
pub struct EbpfAuditSessionConfig {
    pub network_policy: Option<NetworkPolicy>,
    pub file_access_audit: bool,
}

impl EbpfAuditSessionConfig {
    pub fn is_enabled(&self) -> bool {
        self.file_access_audit
            || self
                .network_policy
                .as_ref()
                .map(NetworkPolicy::is_enabled)
                .unwrap_or(false)
    }
}

/// Handle for one execution-scoped eBPF audit session.
pub struct EbpfAuditSession {
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    file_access_collector: Option<Arc<FileAccessCollector>>,
}

impl EbpfAuditSession {
    pub fn initialize(config: EbpfAuditSessionConfig) -> Result<Self, String> {
        if !config.is_enabled() {
            return Err("eBPF audit session requested without enabled features".to_string());
        }

        let network_enforcer = if let Some(network_policy) = config.network_policy.clone() {
            let mut matcher = NetworkPolicyMatcher::new(network_policy.clone());
            matcher.initialize().map_err(|e| e.to_string())?;
            Some(NetworkEnforcer::new(matcher, network_policy.mode()))
        } else {
            None
        };

        let file_access_collector = if config.file_access_audit {
            Some(FileAccessCollector::new())
        } else {
            None
        };

        let (init_tx, init_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));

        let collector_for_thread = file_access_collector.clone();
        let shutdown_for_thread = shutdown.clone();
        // A dedicated synchronous polling thread drains every per-CPU perf ring
        // buffer. We deliberately avoid an async runtime here: the perf fds do
        // not reliably signal epoll readiness in container/emulated runtimes,
        // and async tasks spawned right before the audited process forks can be
        // starved, dropping events. A tight synchronous poll loop on its own
        // thread is robust and simple.
        let thread = thread::spawn(move || {
            run_poll_loop(
                init_tx,
                shutdown_for_thread,
                network_enforcer,
                collector_for_thread,
            );
        });

        init_rx.recv().map_err(|error| {
            format!(
                "Failed to receive eBPF audit session init status: {}",
                error
            )
        })??;

        Ok(Self {
            shutdown,
            thread: Some(thread),
            file_access_collector,
        })
    }

    pub fn finish(mut self) -> Vec<FileAccessRecord> {
        // Let the reader drain the ring buffers before we tear the session
        // down (see wait_for_drain for the quiescence rationale).
        if let Some(collector) = self.file_access_collector.as_deref() {
            wait_for_drain(collector);
        }

        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }

        let records = self
            .file_access_collector
            .as_ref()
            .map(|collector| collector.take())
            .unwrap_or_default();

        if std::env::var_os("CERBERUS_EBPF_DEBUG").is_some() {
            eprintln!(
                "[ebpf-debug] session.finish collected {} file-access records",
                records.len()
            );
        }

        records
    }
}

/// Wait for the perf-buffer reader to finish draining into `collector`.
///
/// The audited command has already exited by the time this runs, so all of its
/// events are sitting in the kernel ring buffers. Rather than a fixed sleep we
/// wait until the collected count stops growing (quiescence), bounded by
/// `DRAIN_MAX_WAIT_MS`, with a `DRAIN_MIN_WAIT_MS` floor so we don't bail out
/// during the initial window before the reader has scheduled its first poll
/// (collector still empty).
fn wait_for_drain(collector: &FileAccessCollector) {
    let poll = std::time::Duration::from_millis(DRAIN_POLL_MS);
    let min_wait = std::time::Duration::from_millis(DRAIN_MIN_WAIT_MS);
    let max_wait = std::time::Duration::from_millis(DRAIN_MAX_WAIT_MS);
    let start = std::time::Instant::now();
    let mut last_len = collector.len();
    let mut stable_rounds = 0u32;
    loop {
        std::thread::sleep(poll);
        let len = collector.len();
        if len == last_len {
            stable_rounds += 1;
        } else {
            stable_rounds = 0;
            last_len = len;
        }
        if start.elapsed() >= min_wait && stable_rounds >= DRAIN_STABLE_ROUNDS {
            break;
        }
        if start.elapsed() >= max_wait {
            break;
        }
    }
}

/// Uniform message for any failure while bringing up the eBPF audit backend.
fn ebpf_init_error(error: impl std::fmt::Display) -> String {
    format!("Failed to initialize eBPF audit backend: {}", error)
}

/// Synchronous reader: load + attach the eBPF programs, open one raw perf
/// buffer per CPU, then poll all of them in a tight loop, dispatching events
/// directly to the collector / network enforcer until shutdown is requested.
/// After shutdown it performs a final drain so events emitted just before the
/// audited process exited are not lost.
fn run_poll_loop(
    init_tx: mpsc::Sender<Result<(), String>>,
    shutdown: Arc<AtomicBool>,
    network_enforcer: Option<NetworkEnforcer>,
    collector: Option<Arc<FileAccessCollector>>,
) {
    use crate::ebpf::EbpfLoader;

    let debug = std::env::var_os("CERBERUS_EBPF_DEBUG").is_some();

    // Bring the backend up as one fallible step so the identical failure
    // handling lives in a single place.
    let setup = (|| -> Result<_, String> {
        let mut loader = EbpfLoader::load().map_err(ebpf_init_error)?;
        loader.attach().map_err(ebpf_init_error)?;
        let bufs = loader
            .open_perf_buffers_raw(PERF_BUFFER_PAGES)
            .map_err(ebpf_init_error)?;
        Ok((loader, bufs))
    })();
    // `_loader` must outlive the poll loop: dropping it detaches the programs.
    let (_loader, mut bufs) = match setup {
        Ok(ready) => ready,
        Err(message) => {
            let _ = init_tx.send(Err(message));
            return;
        }
    };

    // Programs are attached and buffers are open; the session is ready.
    let _ = init_tx.send(Ok(()));
    if debug {
        eprintln!("[ebpf-debug] sync poller ready over {} buffers", bufs.len());
    }

    let mut scratch: Vec<BytesMut> = std::iter::repeat_with(|| BytesMut::with_capacity(4096))
        .take(READ_BATCH)
        .collect();

    let mut total = 0usize;
    loop {
        let drained = drain_once(
            &mut bufs,
            &mut scratch,
            network_enforcer.as_ref(),
            collector.as_deref(),
        );
        total += drained;

        if shutdown.load(Ordering::SeqCst) {
            // Final drain pass to flush anything still buffered.
            let mut extra = drain_once(
                &mut bufs,
                &mut scratch,
                network_enforcer.as_ref(),
                collector.as_deref(),
            );
            while extra > 0 {
                total += extra;
                extra = drain_once(
                    &mut bufs,
                    &mut scratch,
                    network_enforcer.as_ref(),
                    collector.as_deref(),
                );
            }
            break;
        }

        if drained == 0 {
            thread::sleep(std::time::Duration::from_millis(POLL_IDLE_MS));
        }
    }

    if debug {
        eprintln!("[ebpf-debug] sync poller exiting, dispatched {} events", total);
    }
}

/// Read all currently-available events from every per-CPU buffer once.
/// Returns the number of events dispatched.
fn drain_once(
    bufs: &mut [aya::maps::perf::PerfEventArrayBuffer<aya::maps::MapData>],
    scratch: &mut [BytesMut],
    network_enforcer: Option<&NetworkEnforcer>,
    collector: Option<&FileAccessCollector>,
) -> usize {
    use crate::ebpf::EbpfLoader;

    let mut dispatched = 0usize;
    for buf in bufs.iter_mut() {
        loop {
            let events = match buf.read_events(scratch) {
                Ok(e) => e,
                Err(e) => {
                    log::error!("Failed to read perf events: {:?}", e);
                    break;
                }
            };
            if events.read == 0 {
                break;
            }
            for raw in scratch.iter_mut().take(events.read) {
                if let Some(event) = EbpfLoader::decode_event(raw) {
                    dispatch_event(&event, network_enforcer, collector);
                    dispatched += 1;
                }
            }
            if events.read < scratch.len() {
                break;
            }
        }
    }
    dispatched
}

fn dispatch_event(
    event: &EbpfAuditEvent,
    network_enforcer: Option<&NetworkEnforcer>,
    file_access_collector: Option<&FileAccessCollector>,
) {
    match event {
        EbpfAuditEvent::Network(net_event) => {
            if let Some(enforcer) = network_enforcer {
                let result = enforcer.process(net_event, net_event.pid);
                let _ = NetworkEnforcer::to_audit_result(&result);
            }
        }
        EbpfAuditEvent::FileAccess(file_event) => {
            if let Some(collector) = file_access_collector {
                collector.record(file_event.clone());
            }
        }
        _ => {}
    }
}
