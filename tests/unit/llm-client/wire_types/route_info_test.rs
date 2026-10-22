use super::*;

// Test-only builder for `RouteInfo`.
impl RouteInfo {
    fn new(candidate_models: Vec<String>) -> Self {
        Self {
            candidate_models,
            tenant_id: None,
            session_id: None,
        }
    }

    fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

#[test]
fn route_info_builder_sets_tenant_and_session() {
    let info = RouteInfo::new(vec!["gpt-4o".to_string(), "deepseek-chat".to_string()])
        .with_tenant("tenant-1")
        .with_session("session-123");

    assert_eq!(info.candidate_models.len(), 2);
    assert_eq!(info.tenant_id, Some("tenant-1".to_string()));
    assert_eq!(info.session_id, Some("session-123".to_string()));
}
