use super::{
    backend::{Queue, SnapshotStatus},
    capture::SaveSource,
    state::StoredCheckpoint,
    storage::CheckpointStorage,
    CheckpointError, CheckpointUpdateSink, Snapshot, SnapshotId,
};
use crate::LoopState;
use parking_lot::{Mutex, RwLock};
use std::sync::{Arc, Weak};

/// Session-owned driver. Synchronization and the pre-init state stay internal.
#[derive(Default)]
pub struct CheckpointDriver {
    inner: tokio::sync::Mutex<Option<DriverState>>,
    update_sink: RwLock<Option<Weak<dyn CheckpointUpdateSink>>>,
}

struct DriverState {
    queue: Arc<Mutex<Queue>>,
    storage: Arc<dyn CheckpointStorage>,
    hot: bool,
    // Successful physical deletions whose API tombstones can retire on release.
    completed_deletes: Vec<SnapshotId>,
}
pub(super) enum CheckpointCommand {
    Activate,
    Release,
    Save {
        snapshot: Snapshot,
        source: Box<dyn SaveSource>,
    },
    Restore {
        snapshot_id: SnapshotId,
    },
    Delete {
        snapshot_id: SnapshotId,
    },
}
impl CheckpointDriver {
    pub(super) fn new(queue: Arc<Mutex<Queue>>, storage: Arc<dyn CheckpointStorage>) -> Self {
        Self {
            update_sink: RwLock::new(None),
            inner: tokio::sync::Mutex::new(Some(DriverState {
                queue,
                storage,
                hot: false,
                completed_deletes: Vec::new(),
            })),
        }
    }

    /// Bind the current consumer without keeping its event channel alive after
    /// a turn finishes. The caller retains the sink for the duration of the turn.
    pub fn set_update_sink(&self, sink: Arc<dyn CheckpointUpdateSink>) {
        *self.update_sink.write() = Some(Arc::downgrade(&sink));
    }

    /// Attach the first initialized state to the session's existing driver.
    pub(crate) async fn attach(&self, initialized: Self) {
        *self.inner.lock().await = initialized.inner.into_inner();
    }

    pub(super) async fn close(&self) {
        self.inner.lock().await.take();
    }

    /// Execute the queued batch once before the agent loop exits. Requests
    /// arriving during the flush remain queued for the next boundary.
    /// Failure is reported without replacing the agent's exit result.
    /// Returns whether any restore succeeded, even if a later command failed.
    pub async fn flush(&self, state: &mut LoopState, turn_number: &mut u32) -> bool {
        let mut restored = false;
        if let Err(error) = self
            .process_boundary(state, turn_number, false, false, &mut restored)
            .await
        {
            tracing::warn!(%error, "context checkpoint exit flush failed");
        }
        restored
    }

    pub(crate) async fn process_pending(
        &self,
        state: &mut LoopState,
        turn_number: &mut u32,
        append_user_message: bool,
    ) {
        if let Err(error) = self
            .process_boundary(state, turn_number, true, append_user_message, &mut false)
            .await
        {
            tracing::warn!(%error, "context checkpoint failed");
        }
    }

    async fn process_boundary(
        &self,
        state: &mut LoopState,
        turn_number: &mut u32,
        continuing: bool,
        append_user_message: bool,
        restored: &mut bool,
    ) -> Result<(), CheckpointError> {
        // A checkpoint does not carry suspended task state. Keep the entire
        // batch queued until every tool call has a result, including on errors.
        let mut pending = std::collections::HashSet::new();
        {
            let messages = state.messages.read();
            for message in messages.iter() {
                for block in &message.blocks {
                    match block {
                        agent_types::ContentBlock::ToolUse { call_id, .. } => {
                            pending.insert(call_id.clone());
                        }
                        agent_types::ContentBlock::ToolResult { call_id, .. } => {
                            pending.remove(call_id);
                        }
                        _ => {}
                    }
                }
            }
        }
        if !pending.is_empty() {
            return Ok(());
        }
        if let Some(driver) = self.inner.lock().await.as_mut() {
            driver
                .process_pending(
                    state,
                    turn_number,
                    &self.update_sink,
                    continuing,
                    append_user_message,
                    restored,
                )
                .await?;
            let mut queue = driver.queue.lock();
            if driver.hot && queue.accepting && queue.driver_alive {
                queue.capture.publish(state, *turn_number);
            }
        }
        Ok(())
    }
}

impl DriverState {
    async fn process_pending(
        &mut self,
        state: &mut LoopState,
        turn_number: &mut u32,
        update_sink: &RwLock<Option<Weak<dyn CheckpointUpdateSink>>>,
        continuing: bool,
        append_user_message: bool,
        restored: &mut bool,
    ) -> Result<(), CheckpointError> {
        // Bound the batch so continual producers cannot starve the LLM.
        let (count, has_restore) = {
            let queue = self.queue.lock();
            if !queue.driver_alive {
                return Ok(());
            }
            if queue.session_id != state.session_id {
                return Err(CheckpointError::SessionMismatch);
            }
            (
                queue.commands.len(),
                queue
                    .commands
                    .iter()
                    .any(|command| matches!(command, CheckpointCommand::Restore { .. })),
            )
        };
        // Classify once, before any restore can change the turn or history.
        // Each invocation starts at turn 1; suspended resumes append no input.
        let new_input = if has_restore && continuing && *turn_number == 1 && append_user_message {
            state.messages.read().last().cloned()
        } else {
            None
        };
        for _ in 0..count {
            let command = {
                let mut queue = self.queue.lock();
                if !queue.driver_alive {
                    return Ok(());
                }
                queue.commands.pop_front()
            };
            match command {
                Some(CheckpointCommand::Activate) => self.hot = true,
                Some(CheckpointCommand::Release) => {
                    self.hot = false;
                    // Keep the driver and stored snapshots. Give back spare queue
                    // capacity without dropping commands from a later init.
                    let mut queue = self.queue.lock();
                    // Validation and enqueue are atomic. Completed physical
                    // deletions can now retire their invalid-ID records.
                    for id in std::mem::take(&mut self.completed_deletes) {
                        queue.snapshots.shift_remove(&id);
                    }
                    queue.commands.shrink_to_fit();
                    queue.snapshots.shrink_to_fit();
                }
                Some(CheckpointCommand::Save { snapshot, source }) => {
                    debug_assert!(self.hot);
                    let id = snapshot.id.clone();
                    let result = self
                        .storage
                        .save(StoredCheckpoint {
                            snapshot,
                            context: source.capture(state, *turn_number),
                        })
                        .await;
                    let mut queue = self.queue.lock();
                    if let Some(status) = queue.snapshots.get_mut(&id) {
                        // A delete may already have invalidated this pending ID.
                        if *status == SnapshotStatus::Pending {
                            *status = if result.is_ok() {
                                SnapshotStatus::Ready
                            } else {
                                SnapshotStatus::Failed
                            };
                        }
                    }
                    if let Some(evicted) = result? {
                        queue.snapshots.shift_remove(&evicted);
                    }
                }
                Some(CheckpointCommand::Restore { snapshot_id }) => {
                    debug_assert!(self.hot);
                    if self.queue.lock().snapshots.get(&snapshot_id)
                        == Some(&SnapshotStatus::Failed)
                    {
                        return Err(CheckpointError::SnapshotFailed(snapshot_id));
                    }
                    let checkpoint = self
                        .storage
                        .load(&snapshot_id)
                        .await?
                        .ok_or(CheckpointError::SnapshotNotReadyOrNotFound(snapshot_id))?;
                    if checkpoint.snapshot.session_id != state.session_id {
                        return Err(CheckpointError::SessionMismatch);
                    }
                    let queue = self.queue.lock();
                    if !queue.driver_alive {
                        return Ok(());
                    }
                    // Tombstones only reject new requests, not accepted restores.
                    checkpoint
                        .context
                        .restore(state, turn_number, continuing, new_input.as_ref());
                    *restored = true;
                    drop(queue);
                    let sink = update_sink.read().as_ref().and_then(Weak::upgrade);
                    if let Some(sink) = sink {
                        sink.on_restore(
                            &state.session_id,
                            &state.messages.read(),
                            checkpoint.context.todos(),
                        );
                    }
                }
                Some(CheckpointCommand::Delete { snapshot_id }) => {
                    debug_assert!(self.hot);
                    self.storage.delete(&snapshot_id).await?;
                    self.completed_deletes.push(snapshot_id);
                }
                None => break,
            }
        }
        Ok(())
    }
}
impl Drop for DriverState {
    fn drop(&mut self) {
        let mut queue = self.queue.lock();
        queue.driver_alive = false;
        queue.commands.clear();
        queue.capture.clear();
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/core/checkpoint/driver_test.rs"]
mod tests;
