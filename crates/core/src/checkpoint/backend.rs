use super::{
    capture::CaptureStrategy,
    driver::{CheckpointCommand, CheckpointDriver},
    storage::CheckpointStorage,
    types::SnapshotIdGenerator,
    *,
};
use async_trait::async_trait;
use indexmap::IndexMap;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotStatus {
    Pending,
    Ready,
    Failed,
    Deleted,
}

pub(crate) struct Queue {
    pub session_id: String,
    /// API acceptance gate, independent of the driver's queued hot/cold state.
    pub accepting: bool,
    pub driver_alive: bool,
    pub(super) commands: VecDeque<CheckpointCommand>,
    pub snapshots: IndexMap<SnapshotId, SnapshotStatus>,
    id_generator: SnapshotIdGenerator,
    pub(super) capture: Box<dyn CaptureStrategy>,
}
impl Queue {
    pub fn check(&self, context: &SnapshotContext) -> Result<(), CheckpointError> {
        if context.session_id != self.session_id {
            return Err(CheckpointError::SessionMismatch);
        }
        if !self.accepting {
            return Err(CheckpointError::Released);
        }
        if !self.driver_alive {
            return Err(CheckpointError::QueueClosed);
        }
        Ok(())
    }
}

enum Lifecycle {
    Unbound { released: bool },
    Bound(Arc<Mutex<Queue>>),
    Closed,
}
pub(crate) struct QueuedCheckpointBackend {
    storage: Arc<dyn CheckpointStorage>,
    startup_config: CheckpointConfig,
    lifecycle: Mutex<Lifecycle>,
}
impl QueuedCheckpointBackend {
    pub fn new(storage: Arc<dyn CheckpointStorage>, startup_config: CheckpointConfig) -> Self {
        Self {
            storage,
            startup_config,
            lifecycle: Mutex::new(Lifecycle::Unbound { released: false }),
        }
    }
    fn release_for_session(&self, session_id: Option<&str>) {
        let mut lifecycle = self.lifecycle.lock();
        if matches!(*lifecycle, Lifecycle::Closed) {
            return;
        }
        if let Lifecycle::Bound(queue) = &*lifecycle {
            let mut queue = queue.lock();
            if session_id.is_some_and(|id| id != queue.session_id) {
                return;
            }
            if queue.accepting {
                // Close the API gate and append the barrier under the same lock.
                // Commands accepted before this point must still execute.
                queue.accepting = false;
                queue.capture.clear();
                queue.commands.push_back(CheckpointCommand::Release);
            }
        } else if let Lifecycle::Unbound { released, .. } = &mut *lifecycle {
            *released = true;
        }
    }
    /// Terminal session teardown, unlike the reusable handle's release.
    pub(super) fn close(&self) {
        let mut lifecycle = self.lifecycle.lock();
        if let Lifecycle::Bound(queue) = &*lifecycle {
            let mut queue = queue.lock();
            queue.accepting = false;
            queue.driver_alive = false;
            queue.commands.clear();
            queue.snapshots.clear();
            queue.capture.clear();
        }
        *lifecycle = Lifecycle::Closed;
        self.storage.close();
    }

    fn queue(&self) -> Result<Arc<Mutex<Queue>>, CheckpointError> {
        match &*self.lifecycle.lock() {
            Lifecycle::Closed => Err(CheckpointError::QueueClosed),
            Lifecycle::Unbound {
                released: false, ..
            } => Err(CheckpointError::NotInitialized),
            Lifecycle::Unbound { released: true, .. } => Err(CheckpointError::Released),
            Lifecycle::Bound(queue) => Ok(queue.clone()),
        }
    }
}
#[async_trait]
impl CheckpointBackend for QueuedCheckpointBackend {
    async fn init(
        &self,
        context: SnapshotContext,
        config: serde_json::Value,
    ) -> Result<CheckpointBinding, CheckpointError> {
        if !config.is_object() {
            return Err(CheckpointError::InvalidConfig(
                "expected a JSON object".into(),
            ));
        }
        let options: super::config::CheckpointInitOptions = serde_json::from_value(config)
            .map_err(|error| CheckpointError::InvalidConfig(error.to_string()))?;
        let mut lifecycle = self.lifecycle.lock();
        if matches!(*lifecycle, Lifecycle::Closed) {
            return Err(CheckpointError::QueueClosed);
        }
        if let Lifecycle::Bound(queue) = &*lifecycle {
            let mut queue = queue.lock();
            if queue.session_id != context.session_id {
                return Err(CheckpointError::SessionMismatch);
            }
            if !queue.driver_alive {
                return Err(CheckpointError::QueueClosed);
            }
            if queue.accepting {
                return Err(CheckpointError::AlreadyInitialized);
            }
            queue.capture = super::factory::create_capture(
                options.save_mode.unwrap_or(self.startup_config.save_mode),
            );
            queue.commands.push_back(CheckpointCommand::Activate);
            queue.accepting = true;
            return Ok(CheckpointBinding {
                info: BindingInfo {
                    session_id: context.session_id,
                },
                driver: None,
            });
        }
        // Resolve per-binding options only when init assembles the strategy.
        let capture = super::factory::create_capture(
            options.save_mode.unwrap_or(self.startup_config.save_mode),
        );
        let info = BindingInfo {
            session_id: context.session_id.clone(),
        };
        let queue = Arc::new(Mutex::new(Queue {
            session_id: context.session_id,
            accepting: true,
            driver_alive: true,
            commands: VecDeque::from([CheckpointCommand::Activate]),
            snapshots: IndexMap::new(),
            id_generator: SnapshotIdGenerator::default(),
            capture,
        }));
        let driver = CheckpointDriver::new(queue.clone(), self.storage.clone());
        *lifecycle = Lifecycle::Bound(queue);
        Ok(CheckpointBinding {
            info,
            driver: Some(driver),
        })
    }
    fn save(&self, context: SnapshotContext) -> Result<Snapshot, CheckpointError> {
        let queue = self.queue()?;
        let mut queue = queue.lock();
        queue.check(&context)?;
        let source = queue.capture.select()?;
        let snapshot = Snapshot {
            id: queue.id_generator.generate(),
            session_id: queue.session_id.clone(),
        };
        queue
            .snapshots
            .insert(snapshot.id.clone(), SnapshotStatus::Pending);
        queue.commands.push_back(CheckpointCommand::Save {
            snapshot: snapshot.clone(),
            source,
        });
        Ok(snapshot)
    }
    fn list_checkpoint(
        &self,
        context: SnapshotContext,
        access_mode: AccessMode,
        count: Option<i64>,
    ) -> Result<Vec<SnapshotId>, CheckpointError> {
        let queue = self.queue()?;
        let queue = queue.lock();
        queue.check(&context)?;
        let count = count.unwrap_or(10);
        let limit = if count < 0 {
            usize::MAX
        } else {
            count as usize
        };
        match access_mode {
            AccessMode::LastN => Ok(queue
                .snapshots
                .iter()
                .rev()
                .filter(|(_, status)| {
                    matches!(status, SnapshotStatus::Pending | SnapshotStatus::Ready)
                })
                .take(limit)
                .map(|(id, _)| id.clone())
                .collect()),
        }
    }
    async fn restore(
        &self,
        context: SnapshotContext,
        request: RestoreRequest,
    ) -> Result<(), CheckpointError> {
        let queue = self.queue()?;
        let mut queue = queue.lock();
        queue.check(&context)?;
        // ID state and enqueue share one lock. No storage read is needed here:
        // Pending promises are ordered after their Snapshot by this same queue.
        match (request.mode, queue.snapshots.get(&request.snapshot_id)) {
            (_, Some(SnapshotStatus::Ready))
            | (RestoreMode::PromiseMode, Some(SnapshotStatus::Pending)) => {}
            (_, Some(SnapshotStatus::Failed)) => {
                return Err(CheckpointError::SnapshotFailed(request.snapshot_id));
            }
            _ => {
                return Err(CheckpointError::SnapshotNotReadyOrNotFound(
                    request.snapshot_id,
                ))
            }
        }
        queue.commands.push_back(CheckpointCommand::Restore {
            snapshot_id: request.snapshot_id,
        });
        Ok(())
    }
    async fn delete_snapshot(
        &self,
        context: SnapshotContext,
        id: &SnapshotId,
    ) -> Result<(), CheckpointError> {
        let queue = self.queue()?;
        let mut queue = queue.lock();
        queue.check(&context)?;
        if let Some(status) = queue.snapshots.get_mut(id) {
            *status = SnapshotStatus::Deleted;
        }
        queue.commands.push_back(CheckpointCommand::Delete {
            snapshot_id: id.clone(),
        });
        Ok(())
    }
    fn release(&self, context: SnapshotContext) {
        self.release_for_session(Some(&context.session_id));
    }
}
impl Drop for QueuedCheckpointBackend {
    fn drop(&mut self) {
        self.release_for_session(None);
    }
}
