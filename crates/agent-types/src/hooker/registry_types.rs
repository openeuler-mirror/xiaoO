use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HookPointId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookerDescriptor {
    pub id: crate::common::HookerId,
    pub hook_point: HookPointId,
}
