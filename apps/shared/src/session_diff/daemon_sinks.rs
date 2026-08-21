use std::sync::{Arc, Mutex};

use agent_contracts::{LoopEventSink, ToolEventSink};
use agent_types::common::ids::AgentId;
use agent_types::events::{ToolLifecycleEvent, ToolResultEvent};

use super::FileChangeDelta;

/// Receives computed [`FileChangeDelta`]s from the daemon so they can be
/// forwarded to remote clients (e.g. via an SSE channel). The shared crate
/// keeps the transport abstract; the server crate supplies a concrete impl.
pub trait SessionDiffForwarder: Send + Sync {
    fn forward_delta(&self, call_id: &str, delta: FileChangeDelta);
}

/// Wraps an inner [`LoopEventSink`] and a [`SessionDiffTracker`]. On
/// `on_tool_result`, the wrapper runs the same baseline/args computation the
/// TUI uses locally, then forwards the computed delta (if any) via the
/// [`SessionDiffForwarder`] before delegating to the inner sink. All other
/// `LoopEventSink` methods pass through unchanged.
pub struct DiffComputingLoopSink {
    inner: Arc<dyn LoopEventSink>,
    tracker: Arc<Mutex<super::SessionDiffTracker>>,
    forwarder: Arc<dyn SessionDiffForwarder>,
}

impl DiffComputingLoopSink {
    pub fn new(
        inner: Arc<dyn LoopEventSink>,
        tracker: Arc<Mutex<super::SessionDiffTracker>>,
        forwarder: Arc<dyn SessionDiffForwarder>,
    ) -> Self {
        Self {
            inner,
            tracker,
            forwarder,
        }
    }

    fn compute_delta(&self, event: &ToolResultEvent) -> Option<FileChangeDelta> {
        let mut tracker = match self.tracker.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // Mutex poisoning indicates a panic on another thread that
                // held the tracker lock. Surface it so the missing delta is
                // debuggable rather than silently dropped; recover the inner
                // guard so the next tool call does not also fail.
                tracing::error!(
                    target: "session_diff",
                    call_id = %event.call_id,
                    tool = %event.tool_name,
                    is_error = event.is_error,
                    "SessionDiffTracker mutex poisoned; recovering and skipping delta",
                );
                poisoned.into_inner()
            }
        };
        if event.is_error {
            tracker.on_tool_failed(&event.call_id, None)
        } else {
            tracker.on_tool_completed(&event.call_id, &event.tool_name, &event.args_preview, None)
        }
    }
}

impl LoopEventSink for DiffComputingLoopSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        self.inner.on_turn_start(agent_id, turn);
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        self.inner.on_assistant_message(agent_id, text);
    }

    fn on_assistant_reasoning(&self, agent_id: &AgentId, text: &str) {
        self.inner.on_assistant_reasoning(agent_id, text);
    }

    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent) {
        if let Some(delta) = self.compute_delta(event) {
            self.forwarder.forward_delta(&event.call_id, delta);
        }
        self.inner.on_tool_result(agent_id, event);
    }

    fn on_loop_end(&self, agent_id: &AgentId, summary: &agent_types::events::LoopEndSummary) {
        // Drop per-call state accumulated during this turn so the tracker's
        // `tool_file_changes` / `tool_file_baselines` maps do not grow
        // unboundedly across turns. `session_file_changes` (the per-file
        // totals surfaced to the UI) and the session-start content baselines
        // are intentionally retained.
        {
            let mut tracker = match self.tracker.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::error!(
                        target: "session_diff",
                        "SessionDiffTracker mutex poisoned during on_loop_end; \
                         recovering and clearing per-turn state",
                    );
                    poisoned.into_inner()
                }
            };
            tracker.clear_per_turn_state();
        }
        // Drop the tracker guard before delegating to the inner sink so the
        // mutex is not held across arbitrary `on_loop_end` work (e.g. SSE
        // writes, further sink-chain traversal), which could deadlock if the
        // inner path ever needs to touch the tracker.
        self.inner.on_loop_end(agent_id, summary);
    }
}

/// Wraps a [`SessionDiffTracker`] and feeds it `ToolLifecycleEvent::Running`
/// so the tracker can capture a file-content baseline before the tool's
/// executor runs. Other lifecycle variants (`Completed`/`Failed`) are ignored
/// here because the daemon's [`DiffComputingLoopSink`] handles completion via
/// `on_tool_result` (which carries `args_preview` and runs after the
/// executor returns).
pub struct DiffComputingToolSink {
    tracker: Arc<Mutex<super::SessionDiffTracker>>,
}

impl DiffComputingToolSink {
    pub fn new(tracker: Arc<Mutex<super::SessionDiffTracker>>) -> Self {
        Self { tracker }
    }
}

impl ToolEventSink for DiffComputingToolSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        let inner = match event {
            ToolLifecycleEvent::AgentScoped { event, .. } => *event,
            other => other,
        };
        if let ToolLifecycleEvent::Running {
            call_id,
            tool_name,
            args_preview,
        } = inner
        {
            let mut tracker = match self.tracker.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::error!(
                        target: "session_diff",
                        call_id = %call_id,
                        tool = %tool_name,
                        "SessionDiffTracker mutex poisoned during on_tool_running; \
                         recovering and skipping baseline capture",
                    );
                    poisoned.into_inner()
                }
            };
            tracker.on_tool_running(&call_id, &tool_name, &args_preview);
        }
    }
}
