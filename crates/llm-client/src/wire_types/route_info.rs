use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RouteInfo {
    pub candidate_models: Vec<String>,
    pub tenant_id: Option<String>,
    pub session_id: Option<String>,
}

#[allow(dead_code)]
impl RouteInfo {
    pub(crate) fn new(candidate_models: Vec<String>) -> Self {
        Self {
            candidate_models,
            tenant_id: None,
            session_id: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    #[allow(dead_code)]
    pub(crate) fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/wire_types/route_info_test.rs"]
mod tests;
