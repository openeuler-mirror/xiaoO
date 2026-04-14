use agent_contracts::{TraceRecorder, TraceRecorderBuilder};
use agent_types::common::BuildError;
use async_trait::async_trait;
use serde_json::Value;

use super::config::TraceRecorderConfig;
use super::trace_recorder::TraceRecorderImpl;

#[derive(Debug, Clone)]
pub struct TraceRecorderBuilderImpl {
    config: TraceRecorderConfig,
}

#[async_trait]
impl TraceRecorderBuilder for TraceRecorderBuilderImpl {
    fn default() -> Self {
        Self {
            config: TraceRecorderConfig::default(),
        }
    }

    fn from_json(mut self, config: Value) -> Result<Self, BuildError> {
        self.config = TraceRecorderConfig::from_json(config)?;
        Ok(self)
    }

    async fn build(&self) -> Result<Box<dyn TraceRecorder>, BuildError> {
        Ok(Box::new(TraceRecorderImpl::new(&self.config).await?))
    }
}
