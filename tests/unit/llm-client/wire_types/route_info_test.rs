use super::*;

#[test]
fn test_route_info() {
    let info = RouteInfo::new(vec!["gpt-4o".to_string(), "deepseek-chat".to_string()])
        .with_tenant("tenant-1")
        .with_session("session-123");

    assert_eq!(info.candidate_models.len(), 2);
    assert_eq!(info.tenant_id, Some("tenant-1".to_string()));
    assert_eq!(info.session_id, Some("session-123".to_string()));
}
