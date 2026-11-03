//! Save algorithms selected internally at init, independent of queue execution.
use super::{state::ContextState, CheckpointError};
use crate::LoopState;
use std::sync::Arc;

mod current_boundary;
mod next_boundary;
pub(super) use current_boundary::CurrentBoundaryCapture;
pub(super) use next_boundary::NextBoundaryCapture;

pub(super) trait CaptureStrategy: Send {
    fn publish(&mut self, state: &LoopState, turn_number: u32);
    fn select(&self) -> Result<Box<dyn SaveSource>, CheckpointError>;
    fn clear(&mut self);
}

/// A save's selected algorithm and owned state, independent of later publishes.
pub(super) trait SaveSource: Send {
    fn capture(self: Box<Self>, state: &LoopState, turn_number: u32) -> Arc<ContextState>;
}
