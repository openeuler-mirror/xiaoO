use super::{
    backend::QueuedCheckpointBackend,
    capture::{CaptureStrategy, CurrentBoundaryCapture, NextBoundaryCapture},
    storage::{self, StorageFactory},
    CheckpointBackend, CheckpointConfig, CheckpointError, SaveMode,
};
use std::sync::Arc;

/// Construct storage and retain startup defaults; `init` assembles the capture strategy.
pub fn create(config: CheckpointConfig) -> Result<Arc<dyn CheckpointBackend>, CheckpointError> {
    Ok(create_backend(config)?)
}

pub(super) fn create_backend(
    config: CheckpointConfig,
) -> Result<Arc<QueuedCheckpointBackend>, CheckpointError> {
    let storage = storage::factory().create(config.storage.clone())?;
    Ok(Arc::new(QueuedCheckpointBackend::new(storage, config)))
}

pub(super) fn create_capture(mode: SaveMode) -> Box<dyn CaptureStrategy> {
    match mode {
        SaveMode::Next => Box::new(NextBoundaryCapture),
        SaveMode::Current => Box::new(CurrentBoundaryCapture::default()),
    }
}
