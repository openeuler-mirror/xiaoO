use crate::runtime::runtime_view::RuntimeView;
use agent_types::hooker::{HookInvokeError, HookInvokeInput, HookInvokeOutput};
use async_trait::async_trait;
use std::any::Any;

use agent_types::common::HookerId;
use agent_types::hooker::{HookPointId, HookerDescriptor};

#[async_trait]
pub trait Hooker: Send + Sync + 'static {
    fn id(&self) -> &HookerId;

    fn hook_point(&self) -> &HookPointId;

    fn descriptor(&self) -> HookerDescriptor {
        HookerDescriptor {
            id: self.id().clone(),
            hook_point: self.hook_point().clone(),
        }
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError>;

    fn as_any(&self) -> &dyn Any;
}

pub trait HookerRegistry: Send + Sync {
    fn get(&self, id: &HookerId) -> Option<&dyn Hooker>;

    fn list(&self) -> Vec<&dyn Hooker>;

    fn list_for_hook_point(&self, hook_point: &HookPointId) -> Vec<&dyn Hooker>;

    fn is_enabled(&self, id: &HookerId) -> bool;

    fn policy_for(&self, id: &HookerId) -> Option<&serde_json::Value>;
}
