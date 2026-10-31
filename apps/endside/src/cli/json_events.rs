//! Line-delimited CLI progress events, shared by the loop and tool callbacks.
use std::collections::HashSet;
use std::io::Write;
use std::sync::Mutex;

use serde_json::{json, Value};
use xiaoo_api::chat::AgentId;
use xiaoo_api::events::{
    LoopEndSummary, LoopEventSink, ToolEventSink, ToolLifecycleEvent, ToolResultEvent,
};

use super::CliEventSink;

pub(super) struct JsonEventSink {
    session_id: String,
    root_agent: String,
    debug: Option<CliEventSink>,
    state: Mutex<State>,
}

struct State {
    writer: Box<dyn Write + Send>,
    calls: HashSet<(String, String)>,
}

impl JsonEventSink {
    pub(super) fn new(session_id: String, root_agent: String, debug: bool) -> Self {
        Self::with_writer(session_id, root_agent, debug, Box::new(std::io::stdout()))
    }

    fn with_writer(
        session_id: String,
        root_agent: String,
        debug: bool,
        writer: Box<dyn Write + Send>,
    ) -> Self {
        Self {
            session_id,
            root_agent,
            debug: debug.then(CliEventSink::new),
            state: Mutex::new(State {
                writer,
                calls: HashSet::new(),
            }),
        }
    }

    fn write(&self, state: &mut State, typ: &str, agent: &str, mut data: Value) {
        data["session_id"] = json!(self.session_id);
        data["agent_id"] = json!(agent);
        // One lock covers serialization, the newline, and flush so parallel tools
        // and subagents cannot interleave JSON frames.
        let _ = writeln!(state.writer, "{}", json!({"type": typ, "data": data}));
        let _ = state.writer.flush();
    }

    fn call(
        &self,
        state: &mut State,
        agent: &str,
        call_id: &str,
        tool_name: &str,
        args_preview: &str,
        status: &str,
    ) {
        if state.calls.insert((agent.into(), call_id.into())) {
            self.write(state, "tool_call", agent, json!({ "call_id": call_id, "tool_name": tool_name, "args_preview": args_preview, "status": status }));
        }
    }
}

impl LoopEventSink for JsonEventSink {
    fn on_turn_start(&self, agent: &AgentId, turn: u32) {
        self.write(
            &mut self.state.lock().unwrap(),
            "turn_started",
            &agent.0,
            json!({"turn": turn}),
        );
        if let Some(debug) = &self.debug {
            debug.on_turn_start(agent, turn);
        }
    }

    fn on_assistant_message(&self, agent: &AgentId, text: &str) {
        if let Some(debug) = &self.debug {
            debug.on_assistant_message(agent, text);
        }
    }

    fn on_tool_result(&self, agent: &AgentId, event: &ToolResultEvent) {
        let mut state = self.state.lock().unwrap();
        // Invalid/filtered calls can produce a result without a lifecycle start.
        // Still give consumers a matching call, explicitly marked as reported.
        self.call(
            &mut state,
            &agent.0,
            &event.call_id,
            &event.tool_name,
            &event.args_preview,
            "reported",
        );
        self.write(
            &mut state,
            "tool_result",
            &agent.0,
            serde_json::to_value(event).unwrap(),
        );
        drop(state);
        if let Some(debug) = &self.debug {
            debug.on_tool_result(agent, event);
        }
    }

    fn on_loop_end(&self, agent: &AgentId, summary: &LoopEndSummary) {
        self.write(
            &mut self.state.lock().unwrap(),
            "loop_end",
            &agent.0,
            serde_json::to_value(summary).unwrap(),
        );
        if let Some(debug) = &self.debug {
            debug.on_loop_end(agent, summary);
        }
    }
}

impl ToolEventSink for JsonEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        let mut agent = self.root_agent.clone();
        let mut event = event;
        while let ToolLifecycleEvent::AgentScoped {
            agent_id,
            event: inner,
        } = event
        {
            agent = agent_id.0;
            event = *inner;
        }
        let (call_id, tool_name, args_preview, status) = match event {
            ToolLifecycleEvent::Pending {
                call_id,
                tool_name,
                args_preview,
            } => (call_id, tool_name, args_preview, "pending"),
            ToolLifecycleEvent::Running {
                call_id,
                tool_name,
                args_preview,
            } => (call_id, tool_name, args_preview, "running"),
            ToolLifecycleEvent::Denied {
                call_id,
                tool_name,
                args_preview,
                ..
            } => (call_id, tool_name, args_preview, "denied"),
            ToolLifecycleEvent::Failed {
                call_id,
                tool_name,
                args_preview,
                ..
            } => (call_id, tool_name, args_preview, "failed"),
            ToolLifecycleEvent::Completed {
                call_id,
                tool_name,
                args_preview,
            } => (call_id, tool_name, args_preview, "completed"),
            ToolLifecycleEvent::AgentScoped { .. } => unreachable!(),
        };
        self.call(
            &mut self.state.lock().unwrap(),
            &agent,
            &call_id,
            &tool_name,
            &args_preview,
            status,
        );
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/cli/json_events_test.rs"]
mod tests;
