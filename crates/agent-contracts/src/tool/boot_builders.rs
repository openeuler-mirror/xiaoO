use crate::tool::{ToolRegistry, ToolSource, ToolStateStore};
use agent_types::common::BuildError;
use agent_types::tool::{ToolRegistryConfig, ToolStateStoreConfig};

pub trait ToolRegistryBuilder: Send {
    fn with_sources(self, sources: Vec<Box<dyn ToolSource>>) -> Self
    where
        Self: Sized;

    fn with_config(self, config: ToolRegistryConfig) -> Self
    where
        Self: Sized;

    fn build(self) -> Result<Box<dyn ToolRegistry>, BuildError>
    where
        Self: Sized;
}

pub trait ToolStateStoreBuilder: Send {
    fn with_config(self, config: ToolStateStoreConfig) -> Self
    where
        Self: Sized;

    fn build(self) -> Result<Box<dyn ToolStateStore>, BuildError>
    where
        Self: Sized;
}
