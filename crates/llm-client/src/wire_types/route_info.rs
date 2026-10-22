use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct RouteInfo {
    pub candidate_models: Vec<String>,
    pub tenant_id: Option<String>,
    pub session_id: Option<String>,
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/wire_types/route_info_test.rs"]
mod tests;
