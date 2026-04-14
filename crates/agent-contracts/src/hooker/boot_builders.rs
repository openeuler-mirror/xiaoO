use crate::hooker::registry::HookerRegistry;
use agent_types::common::BuildError;
use agent_types::hooker::HookerRegistryConfig;

pub trait HookerRegistryBuilder: Send {
    fn with_config(self, config: HookerRegistryConfig) -> Self
    where
        Self: Sized;

    fn build(self) -> Result<Box<dyn HookerRegistry>, BuildError>
    where
        Self: Sized;
}
