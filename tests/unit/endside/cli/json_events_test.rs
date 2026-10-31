use super::*;
use std::sync::Arc;

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    flushes: usize,
}
struct Writer(Arc<Mutex<Capture>>);
impl Write for Writer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().bytes.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.lock().unwrap().flushes += 1;
        Ok(())
    }
}
fn sink() -> (Arc<JsonEventSink>, Arc<Mutex<Capture>>) {
    let capture = Arc::new(Mutex::new(Capture::default()));
    (
        Arc::new(JsonEventSink::with_writer(
            "session-1".into(),
            "root".into(),
            false,
            Box::new(Writer(capture.clone())),
        )),
        capture,
    )
}
fn events(capture: &Arc<Mutex<Capture>>) -> Vec<Value> {
    let captured = capture.lock().unwrap();
    let text = std::str::from_utf8(&captured.bytes).unwrap();
    assert!(text.ends_with('\n'));
    let events: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(captured.flushes, events.len());
    events
}
fn result(id: &str, error: bool) -> ToolResultEvent {
    ToolResultEvent {
        call_id: id.into(),
        tool_name: "bash".into(),
        args_preview: "echo 中文\n".into(),
        output_preview: "中文\nresult".into(),
        is_error: error,
    }
}

#[test]
fn streams_turn_call_result_and_loop_end_in_order() {
    let (sink, capture) = sink();
    let agent = AgentId("root".into());
    sink.on_turn_start(&agent, 1);
    sink.emit(ToolLifecycleEvent::Running {
        call_id: "c1".into(),
        tool_name: "bash".into(),
        args_preview: "echo 中文\n".into(),
    });
    // Start is visible before the result arrives.
    assert_eq!(events(&capture).len(), 2);
    sink.emit(ToolLifecycleEvent::Completed {
        call_id: "c1".into(),
        tool_name: "bash".into(),
        args_preview: "echo 中文\n".into(),
    });
    sink.on_tool_result(&agent, &result("c1", false));
    sink.on_loop_end(
        &agent,
        &LoopEndSummary {
            turn_count: 1,
            total_tokens: 42,
            stop_reason: "complete".into(),
        },
    );
    let e = events(&capture);
    assert_eq!(
        e.iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["turn_started", "tool_call", "tool_result", "loop_end"]
    );
    for event in &e {
        assert_eq!(event["data"]["session_id"], "session-1");
        assert_eq!(event["data"]["agent_id"], "root");
    }
    assert_eq!(e[0]["data"]["turn"], 1);
    assert_eq!(e[1]["data"]["call_id"], e[2]["data"]["call_id"]);
    assert_eq!(e[1]["data"]["status"], "running");
    assert_eq!(e[2]["data"]["output_preview"], "中文\nresult");
    assert_eq!(e[2]["data"]["is_error"], false);
}

#[test]
fn denied_and_unstarted_results_have_matching_calls() {
    let (sink, capture) = sink();
    sink.emit(ToolLifecycleEvent::Denied {
        call_id: "denied".into(),
        tool_name: "bash".into(),
        args_preview: "echo 中文\n".into(),
        reason: "denied".into(),
    });
    sink.on_tool_result(&AgentId("root".into()), &result("denied", true));
    sink.on_tool_result(&AgentId("root".into()), &result("invalid", true));
    let e = events(&capture);
    assert_eq!(e.len(), 4);
    assert_eq!(e[0]["data"]["status"], "denied");
    assert_eq!(e[1]["data"]["is_error"], true);
    assert_eq!(e[2]["type"], "tool_call");
    assert_eq!(e[2]["data"]["status"], "reported");
    assert_eq!(e[3]["data"]["call_id"], "invalid");
}

#[test]
fn parallel_agents_keep_frames_intact_and_call_ids_independent() {
    let (sink, capture) = sink();
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let sink = sink.clone();
            std::thread::spawn(move || {
                let agent = AgentId(format!("child-{i}"));
                sink.emit(
                    ToolLifecycleEvent::Pending {
                        call_id: "same-id".into(),
                        tool_name: "bash".into(),
                        args_preview: "echo 中文\n".into(),
                    }
                    .scoped(agent.clone()),
                );
                sink.emit(
                    ToolLifecycleEvent::Running {
                        call_id: "same-id".into(),
                        tool_name: "bash".into(),
                        args_preview: "echo 中文\n".into(),
                    }
                    .scoped(agent.clone()),
                );
                sink.on_tool_result(&agent, &result("same-id", false));
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let e = events(&capture);
    assert_eq!(e.len(), 16);
    for i in 0..8 {
        let agent = format!("child-{i}");
        let own: Vec<_> = e
            .iter()
            .filter(|e| e["data"]["agent_id"] == agent)
            .collect();
        assert_eq!(own.len(), 2);
        assert_eq!(own[0]["type"], "tool_call");
        assert_eq!(own[1]["type"], "tool_result");
    }
}
