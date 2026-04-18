//! Network policy enforcement via eBPF.
//!
//! This module provides network policy enforcement using eBPF monitoring.
//! It is only available when the `ebpf` feature is enabled.

use crate::audit::EbpfAuditEvent;
use crate::network::{NetworkEnforcer, NetworkPolicyMatcher};
use crate::policy::NetworkPolicy;

/// Handle for the network enforcement thread.
pub struct NetworkEnforcementHandle {
    _thread_handle: std::thread::JoinHandle<()>,
}

/// Initialize network policy enforcement.
///
/// This spawns a background thread that:
/// 1. Loads and attaches eBPF tracepoints for network monitoring
/// 2. Receives network events from the kernel
/// 3. Evaluates each event against the network policy
/// 4. Enforces policy by killing processes that violate the policy (in enforce mode)
///
/// # Errors
///
/// Returns an error if the eBPF backend cannot be initialized.
pub fn initialize_network_enforcement(
    network_policy: NetworkPolicy,
) -> Result<NetworkEnforcementHandle, String> {
    let mut matcher = NetworkPolicyMatcher::new(network_policy.clone());
    matcher.initialize().map_err(|e| e.to_string())?;

    let mode = network_policy.mode();
    let enforcer = NetworkEnforcer::new(matcher, mode);
    let (init_tx, init_rx) = std::sync::mpsc::channel();

    let thread_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async move {
            let mut backend = match crate::ebpf::EbpfAuditBackend::new() {
                Ok(backend) => backend,
                Err(error) => {
                    let _ = init_tx.send(Err(format!(
                        "Failed to initialize eBPF audit backend: {}",
                        error
                    )));
                    return;
                }
            };

            let _ = init_tx.send(Ok(()));

            while let Some(event) = backend.next_event().await {
                if let EbpfAuditEvent::Network(ref net_event) = event {
                    let result = enforcer.process(net_event, net_event.pid);
                    // Update the event result for logging
                    let _ = NetworkEnforcer::to_audit_result(&result);
                }
            }
        });
    });

    init_rx.recv().map_err(|error| {
        format!(
            "Failed to receive network enforcement init status: {}",
            error
        )
    })??;

    Ok(NetworkEnforcementHandle {
        _thread_handle: thread_handle,
    })
}
