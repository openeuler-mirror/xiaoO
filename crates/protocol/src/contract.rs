//! Machine-readable daemon protocol contract.

use schemars::schema_for;
use serde_json::{json, Value};

use crate::response::{
    DaemonReadyMessage, GatewayCapabilitiesResponse, GatewayErrorResponse, GatewayHealthResponse,
    RuntimeCheckoutResponse, RuntimeCheckpointResponse, RuntimeCheckpointSnapshotDeleteResponse,
    RuntimeExecInterruptedResponse, RuntimeExecResponse, RuntimeOpenResponse, RuntimePauseResponse,
    RuntimeReadFileResponse, RuntimeResumeResponse, RuntimeWriteFileResponse,
};
use crate::sse::RuntimeSseEvent;
use crate::wire::{
    RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, RuntimeTurnRequest,
};

/// Current daemon wire protocol version.
pub const PROTOCOL_VERSION: u32 = 1;

/// Current layout version of the machine-readable protocol artifact.
pub const PROTOCOL_ARTIFACT_VERSION: u32 = 1;

/// Build the versioned JSON Schema bundle consumed by external clients.
pub fn protocol_contract() -> Value {
    json!({
        "artifact_version": PROTOCOL_ARTIFACT_VERSION,
        "protocol_version": PROTOCOL_VERSION,
        "schemas": {
            "runtime_open_request": schema_for!(RuntimeOpenRequest),
            "runtime_turn_request": schema_for!(RuntimeTurnRequest),
            "runtime_close_request": schema_for!(RuntimeCloseRequest),
            "runtime_cancel_request": schema_for!(RuntimeCancelRequest),
            "runtime_heartbeat_request": schema_for!(RuntimeHeartbeatRequest),
            "runtime_detach_request": schema_for!(RuntimeDetachRequest),
            "runtime_interaction_request": schema_for!(RuntimeInteractionRequest),
            "runtime_sse_event": schema_for!(RuntimeSseEvent),
            "daemon_ready_message": schema_for!(DaemonReadyMessage),
            "gateway_health_response": schema_for!(GatewayHealthResponse),
            "gateway_capabilities_response": schema_for!(GatewayCapabilitiesResponse),
            "gateway_error_response": schema_for!(GatewayErrorResponse),
            "runtime_open_response": schema_for!(RuntimeOpenResponse),
            "runtime_checkpoint_response": schema_for!(RuntimeCheckpointResponse),
            "runtime_checkout_response": schema_for!(RuntimeCheckoutResponse),
            "runtime_pause_response": schema_for!(RuntimePauseResponse),
            "runtime_resume_response": schema_for!(RuntimeResumeResponse),
            "runtime_checkpoint_snapshot_delete_response": schema_for!(RuntimeCheckpointSnapshotDeleteResponse),
            "runtime_exec_response": schema_for!(RuntimeExecResponse),
            "runtime_exec_interrupted_response": schema_for!(RuntimeExecInterruptedResponse),
            "runtime_read_file_response": schema_for!(RuntimeReadFileResponse),
            "runtime_write_file_response": schema_for!(RuntimeWriteFileResponse),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{protocol_contract, PROTOCOL_ARTIFACT_VERSION, PROTOCOL_VERSION};

    #[test]
    fn contract_contains_all_runtime_wire_schemas() {
        let contract = protocol_contract();
        assert_eq!(contract["artifact_version"], PROTOCOL_ARTIFACT_VERSION);
        assert_eq!(contract["protocol_version"], PROTOCOL_VERSION);

        let schemas = contract["schemas"].as_object().expect("schemas object");
        assert_eq!(schemas.len(), 22);
        assert!(schemas.contains_key("runtime_open_request"));
        assert!(schemas.contains_key("runtime_turn_request"));
        assert!(schemas.contains_key("runtime_close_request"));
        assert!(schemas.contains_key("runtime_cancel_request"));
        assert!(schemas.contains_key("runtime_heartbeat_request"));
        assert!(schemas.contains_key("runtime_detach_request"));
        assert!(schemas.contains_key("runtime_interaction_request"));
        assert!(schemas.contains_key("runtime_sse_event"));
        assert!(schemas.contains_key("daemon_ready_message"));
        assert!(schemas.contains_key("gateway_health_response"));
        assert!(schemas.contains_key("gateway_capabilities_response"));
        assert!(schemas.contains_key("gateway_error_response"));
        assert!(schemas.contains_key("runtime_open_response"));
        assert!(schemas.contains_key("runtime_checkpoint_response"));
        assert!(schemas.contains_key("runtime_checkout_response"));
        assert!(schemas.contains_key("runtime_pause_response"));
        assert!(schemas.contains_key("runtime_resume_response"));
        assert!(schemas.contains_key("runtime_checkpoint_snapshot_delete_response"));
        assert!(schemas.contains_key("runtime_exec_response"));
        assert!(schemas.contains_key("runtime_exec_interrupted_response"));
        assert!(schemas.contains_key("runtime_read_file_response"));
        assert!(schemas.contains_key("runtime_write_file_response"));
    }
}
