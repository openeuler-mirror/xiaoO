//! Machine-readable daemon protocol contract.

use schemars::schema_for;
use serde_json::{json, Value};

use crate::response::{
    CronCatalogResponse, CronRunResponse, DaemonReadyMessage, GatewayCapabilitiesResponse,
    GatewayErrorResponse, GatewayHealthResponse, RuntimeCatalogResponse, RuntimeCheckoutResponse,
    RuntimeCheckpointCatalogResponse, RuntimeCheckpointResponse,
    RuntimeCheckpointSnapshotDeleteResponse, RuntimeExecInterruptedResponse, RuntimeExecResponse,
    RuntimeExportResponse, RuntimeOpenResponse, RuntimePauseResponse, RuntimeReadFileResponse,
    RuntimeResumeResponse, RuntimeWriteFileResponse, SandboxCatalogResponse,
};
use crate::sse::RuntimeSseEvent;
use crate::wire::{
    CronRunRequest, RuntimeCancelRequest, RuntimeCheckoutRequest, RuntimeCheckpointRequest,
    RuntimeCheckpointSnapshotDeleteRequest, RuntimeCloseRequest, RuntimeDetachRequest,
    RuntimeExecRequest, RuntimeExportRequest, RuntimeHeartbeatRequest, RuntimeInteractionRequest,
    RuntimeOpenRequest, RuntimePauseRequest, RuntimeReadFileRequest, RuntimeResumeRequest,
    RuntimeTurnRequest, RuntimeWriteFileRequest,
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
            "runtime_checkpoint_request": schema_for!(RuntimeCheckpointRequest),
            "runtime_checkout_request": schema_for!(RuntimeCheckoutRequest),
            "runtime_pause_request": schema_for!(RuntimePauseRequest),
            "runtime_resume_request": schema_for!(RuntimeResumeRequest),
            "runtime_checkpoint_snapshot_delete_request": schema_for!(RuntimeCheckpointSnapshotDeleteRequest),
            "runtime_exec_request": schema_for!(RuntimeExecRequest),
            "runtime_read_file_request": schema_for!(RuntimeReadFileRequest),
            "runtime_write_file_request": schema_for!(RuntimeWriteFileRequest),
            "runtime_export_request": schema_for!(RuntimeExportRequest),
            "cron_run_request": schema_for!(CronRunRequest),
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
            "runtime_export_response": schema_for!(RuntimeExportResponse),
            "runtime_catalog_response": schema_for!(RuntimeCatalogResponse),
            "runtime_checkpoint_catalog_response": schema_for!(RuntimeCheckpointCatalogResponse),
            "sandbox_catalog_response": schema_for!(SandboxCatalogResponse),
            "cron_catalog_response": schema_for!(CronCatalogResponse),
            "cron_run_response": schema_for!(CronRunResponse),
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
        assert_eq!(schemas.len(), 38);
        assert!(schemas.contains_key("runtime_open_request"));
        assert!(schemas.contains_key("runtime_turn_request"));
        assert!(schemas.contains_key("sandbox_catalog_response"));
        assert!(schemas.contains_key("runtime_close_request"));
        assert!(schemas.contains_key("runtime_cancel_request"));
        assert!(schemas.contains_key("runtime_heartbeat_request"));
        assert!(schemas.contains_key("runtime_detach_request"));
        assert!(schemas.contains_key("runtime_interaction_request"));
        assert!(schemas.contains_key("runtime_checkpoint_request"));
        assert!(schemas.contains_key("runtime_checkout_request"));
        assert!(schemas.contains_key("runtime_pause_request"));
        assert!(schemas.contains_key("runtime_resume_request"));
        assert!(schemas.contains_key("runtime_checkpoint_snapshot_delete_request"));
        assert!(schemas.contains_key("runtime_exec_request"));
        assert!(schemas.contains_key("runtime_read_file_request"));
        assert!(schemas.contains_key("runtime_write_file_request"));
        assert!(schemas.contains_key("runtime_export_request"));
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
        assert!(schemas.contains_key("runtime_export_response"));
        assert!(schemas.contains_key("runtime_catalog_response"));
        assert!(schemas.contains_key("runtime_checkpoint_catalog_response"));
        assert!(schemas.contains_key("cron_run_request"));
        assert!(schemas.contains_key("cron_catalog_response"));
        assert!(schemas.contains_key("cron_run_response"));
    }
}
