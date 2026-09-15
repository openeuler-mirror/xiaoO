use super::McpTransport;
use crate::McpError;
use async_trait::async_trait;
use serde_json::Value;

struct LegacyTransportDefaults;

#[async_trait]
impl McpTransport for LegacyTransportDefaults {
    async fn send_request(
        &self,
        _id: u64,
        _method: &str,
        _params: Option<Value>,
    ) -> Result<Value, McpError> {
        unreachable!()
    }

    async fn send_notification(
        &self,
        _method: &str,
        _params: Option<Value>,
    ) -> Result<(), McpError> {
        unreachable!()
    }
}

#[test]
fn legacy_transports_keep_the_original_protocol_version_contract() {
    let transport = LegacyTransportDefaults;

    assert_eq!(transport.initialize_protocol_version(), "2024-11-05");
    assert!(transport
        .validate_negotiated_protocol_version("2024-11-05")
        .is_ok());
    assert!(transport
        .validate_negotiated_protocol_version("2025-11-25")
        .is_ok());
}
