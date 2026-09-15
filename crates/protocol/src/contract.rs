//! Machine-readable daemon protocol contract.

use schemars::schema_for;
use serde_json::{json, Value};

use crate::response::{
    ChannelCatalogResponse, ChannelTestResponse, CronCatalogResponse, CronRunResponse,
    DaemonReadyMessage, GatewayCapabilitiesResponse, GatewayErrorResponse, GatewayHealthResponse,
    RuntimeCatalogResponse, RuntimeCheckoutResponse, RuntimeCheckpointCatalogResponse,
    RuntimeCheckpointResponse, RuntimeCheckpointSnapshotDeleteResponse,
    RuntimeExecInterruptedResponse, RuntimeExecResponse, RuntimeExportResponse,
    RuntimeOpenResponse, RuntimePauseResponse, RuntimeReadFileResponse, RuntimeResumeResponse,
    RuntimeWriteFileResponse, SandboxCatalogResponse,
};
use crate::sse::RuntimeSseEvent;
use crate::wire::{
    ChannelTestRequest, CronRunRequest, RuntimeCancelRequest, RuntimeCheckoutRequest,
    RuntimeCheckpointRequest, RuntimeCheckpointSnapshotDeleteRequest, RuntimeCloseRequest,
    RuntimeDetachRequest, RuntimeExecRequest, RuntimeExportRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, RuntimePauseRequest, RuntimeReadFileRequest,
    RuntimeResumeRequest, RuntimeTurnRequest, RuntimeWriteFileRequest,
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
            "channel_test_request": schema_for!(ChannelTestRequest),
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
            "channel_catalog_response": schema_for!(ChannelCatalogResponse),
            "channel_test_response": schema_for!(ChannelTestResponse),
        }
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/protocol/contract_test.rs"]
mod tests;
