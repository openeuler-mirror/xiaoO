use super::{CaptureStrategy, CheckpointError, ContextState, LoopState, SaveSource};
use std::sync::Arc;

#[derive(Default)]
pub(in crate::checkpoint) struct CurrentBoundaryCapture {
    current: Option<Arc<ContextState>>,
}
struct CurrentBoundarySave {
    context: Arc<ContextState>,
}

impl CaptureStrategy for CurrentBoundaryCapture {
    fn publish(&mut self, state: &LoopState, turn_number: u32) {
        self.current = Some(Arc::new(ContextState::capture(state, turn_number)));
    }

    fn select(&self) -> Result<Box<dyn SaveSource>, CheckpointError> {
        let context = self
            .current
            .as_ref()
            .ok_or(CheckpointError::NoCurrentCheckpoint)?
            .clone();
        Ok(Box::new(CurrentBoundarySave { context }))
    }

    fn clear(&mut self) {
        self.current = None;
    }
}

impl SaveSource for CurrentBoundarySave {
    fn capture(self: Box<Self>, _: &LoopState, _: u32) -> Arc<ContextState> {
        self.context
    }
}
