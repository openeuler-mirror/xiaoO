use super::{protocol_contract, PROTOCOL_ARTIFACT_VERSION, PROTOCOL_VERSION};

#[test]
fn contract_contains_all_runtime_wire_schemas() {
    let contract = protocol_contract();
    assert_eq!(contract["artifact_version"], PROTOCOL_ARTIFACT_VERSION);
    assert_eq!(contract["protocol_version"], PROTOCOL_VERSION);

    let schemas = contract["schemas"].as_object().expect("schemas object");
    assert_eq!(schemas.len(), 41);
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
    assert!(schemas.contains_key("channel_test_request"));
    assert!(schemas.contains_key("channel_catalog_response"));
    assert!(schemas.contains_key("channel_test_response"));
}
