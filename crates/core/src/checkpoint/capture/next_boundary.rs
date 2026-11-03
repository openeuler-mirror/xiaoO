use super::{CaptureStrategy, CheckpointError, ContextState, LoopState, SaveSource};
use std::sync::Arc;

pub(in crate::checkpoint) struct NextBoundaryCapture;
struct NextBoundarySave;

impl CaptureStrategy for NextBoundaryCapture {
    fn publish(&mut self, _: &LoopState, _: u32) {}

    fn select(&self) -> Result<Box<dyn SaveSource>, CheckpointError> {
        Ok(Box::new(NextBoundarySave))
    }

    fn clear(&mut self) {}
}

impl SaveSource for NextBoundarySave {
    fn capture(self: Box<Self>, state: &LoopState, turn_number: u32) -> Arc<ContextState> {
        Arc::new(ContextState::capture(state, turn_number))
    }
}
