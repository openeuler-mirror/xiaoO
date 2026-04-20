use crate::hook::registry::HookerRegistry;
use agent_types::common::BuildError;
use agent_types::hook::HookerRegistryConfig;

pub trait HookerRegistryBuilder: Send {
    fn with_config(self, config: HookerRegistryConfig) -> Self
    where
        Self: Sized;

    fn build(self) -> Result<Box<dyn HookerRegistry>, BuildError>
    where
        Self: Sized;
}
