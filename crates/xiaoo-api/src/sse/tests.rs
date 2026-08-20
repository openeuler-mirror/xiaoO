//! SSE 兼容性测试：JSON 样本提取自 daemon `apps/serverside/src/httpserver/sse_sink.rs`
//! 的 `SseStreamEvent` 序列化输出。每个测试断言 `RuntimeSseEvent` 能
//! round-trip 反序列化 + 重新序列化为相同 JSON。

#![cfg(test)]

use super::*;
use serde_json::Value;

/// 断言一个 JSON 字符串能反序列化为 `RuntimeSseEvent`，且重新序列化后
/// 与原 JSON 字符串语义相等（按 `serde_json::Value` 比较，避免字段顺序问题）。
fn assert_round_trip(json: &str, expected_variant_name: &str) {
    let event: RuntimeSseEvent = serde_json::from_str(json)
        .unwrap_or_else(|e| panic!("should deserialize {json}: {e}"));
    let reserialized = serde_json::to_string(&event)
        .unwrap_or_else(|e| panic!("should re-serialize: {e}"));
    let original: Value = serde_json::from_str(json).expect("original should be valid JSON");
    let roundtrip: Value =
        serde_json::from_str(&reserialized).expect("re-serialized should be valid JSON");
    assert_eq!(
        original, roundtrip,
        "round-trip mismatch for {expected_variant_name}:\n  original: {json}\n  roundtrip: {reserialized}"
    );
    // 验证 type 字段
    assert_eq!(
        original["type"], roundtrip["type"],
        "type field mismatch for {expected_variant_name}"
    );
}

/// 断言 `parse_sse_data` 对已知事件返回 `Ok(Some(event))`。
fn assert_parses(json: &str) -> RuntimeSseEvent {
    parse_sse_data(json)
        .unwrap_or_else(|e| panic!("parse_sse_data should not error for {json}: {e}"))
        .unwrap_or_else(|| panic!("parse_sse_data should return Some for {json}"))
}

/// 断言 `parse_sse_data` 对未知事件/空载荷返回 `Ok(None)`。
fn assert_returns_none(json: &str) {
    assert!(
        parse_sse_data(json)
            .unwrap_or_else(|e| panic!("should not error: {e}"))
            .is_none(),
        "parse_sse_data should return None for {json}"
    );
}

// ===========================================================================
// 各事件变体的 round-trip 测试（JSON 样本对照 daemon sse_sink.rs 序列化）
// ===========================================================================

#[test]
fn turn_start_round_trip() {
    assert_round_trip(
        r#"{"type":"turn_start","agent_id":"root","turn":1}"#,
        "TurnStart",
    );
}

#[test]
fn text_delta_round_trip() {
    assert_round_trip(
        r#"{"type":"text_delta","agent_id":"root","delta":"Hello","snapshot":"Hello"}"#,
        "TextDelta",
    );
}

#[test]
fn thinking_delta_round_trip() {
    assert_round_trip(
        r#"{"type":"thinking_delta","agent_id":"root","delta":"Hmm","snapshot":"Hmm"}"#,
        "ThinkingDelta",
    );
}

#[test]
fn tool_result_round_trip() {
    assert_round_trip(
        r#"{"type":"tool_result","agent_id":"root","call_id":"c1","tool_name":"bash","output_preview":"ok","is_error":false,"args_preview":"{}"}"#,
        "ToolResult",
    );
}

#[test]
fn tool_file_change_round_trip() {
    assert_round_trip(
        r#"{"type":"tool_file_change","call_id":"c1","file_path":"a.txt","additions":1,"deletions":0}"#,
        "ToolFileChange",
    );
}

#[test]
fn plan_update_round_trip() {
    assert_round_trip(
        r#"{"type":"plan_update","title":"Plan","items":[{"status":"pending","content":"do thing"}]}"#,
        "PlanUpdate",
    );
}

#[test]
fn subagent_spawn_round_trip() {
    // parent_agent_id = Some
    assert_round_trip(
        r#"{"type":"subagent_spawn","agent_id":"child","parent_agent_id":"root","title":"Sub","description":"desc","task_goal":"goal"}"#,
        "SubagentSpawn",
    );
    // parent_agent_id = None
    assert_round_trip(
        r#"{"type":"subagent_spawn","agent_id":"child","parent_agent_id":null,"title":"Sub","description":"desc","task_goal":"goal"}"#,
        "SubagentSpawn (no parent)",
    );
}

#[test]
fn tool_call_round_trip() {
    assert_round_trip(
        r#"{"type":"tool_call","agent_id":"root","call_id":"c1","tool_name":"bash","args_preview":"{}","status":"running","detail":""}"#,
        "ToolCall",
    );
}

#[test]
fn loop_end_round_trip() {
    assert_round_trip(
        r#"{"type":"loop_end","agent_id":"root","turn_count":1,"total_tokens":100,"stop_reason":"end_turn"}"#,
        "LoopEnd",
    );
}

#[test]
fn interaction_requested_round_trip() {
    // InteractionRequest 是内部 tagged（tag = "kind"），Confirm 变体 + source = None
    // 注：daemon 对 None 字段不 skip，序列化为 "source":null（对照
    // agent_types/src/interaction/types.rs 的 InteractionRequest 定义）。
    assert_round_trip(
        r#"{"type":"interaction_requested","request":{"kind":"confirm","prompt":"Allow?","source":null}}"#,
        "InteractionRequested (Confirm)",
    );
    // TextInput 变体 + is_secret = false（source: null）
    assert_round_trip(
        r#"{"type":"interaction_requested","request":{"kind":"text_input","prompt":"Name?","source":null,"is_secret":false}}"#,
        "InteractionRequested (TextInput)",
    );
    // Choice 变体（source: null）
    assert_round_trip(
        r#"{"type":"interaction_requested","request":{"kind":"choice","prompt":"Pick:","options":["a","b"],"allow_custom_input":false,"source":null}}"#,
        "InteractionRequested (Choice)",
    );
}

#[test]
fn done_round_trip() {
    // session_id 字段重命名为 runtime_id（对照 daemon SseStreamEvent::Done）
    let json = r#"{"type":"done","reply":"Hi","raw_reply":"Hi","conversation_id":"conv","runtime_id":"s1","turn_count":1,"total_tokens":100,"prompt_tokens":10,"completion_tokens":5,"estimated_input_tokens":8,"messages":[],"stop_reason":"end_turn","actions":[]}"#;
    assert_round_trip(json, "Done");

    // 关键：runtime_id 字段名（不是 session_id）
    let event = assert_parses(json);
    match &event {
        RuntimeSseEvent::Done { session_id, .. } => {
            assert_eq!(session_id, "s1");
        }
        _ => panic!("expected Done"),
    }

    // 重新序列化后字段名仍是 runtime_id
    let reserialized = serde_json::to_string(&event).unwrap();
    let value: Value = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(value["runtime_id"], "s1");
    assert!(
        value.get("session_id").is_none(),
        "session_id field should NOT appear in serialized Done (only runtime_id)"
    );
}

#[test]
fn error_round_trip() {
    assert_round_trip(
        r#"{"type":"error","error":"boom"}"#,
        "Error",
    );
}

#[test]
fn cancelled_round_trip() {
    // session_id 字段重命名为 runtime_id（对照 daemon SseStreamEvent::Cancelled）
    let json = r#"{"type":"cancelled","runtime_id":"s1"}"#;
    assert_round_trip(json, "Cancelled");

    let event = assert_parses(json);
    match &event {
        RuntimeSseEvent::Cancelled { session_id } => {
            assert_eq!(session_id, "s1");
        }
        _ => panic!("expected Cancelled"),
    }

    let reserialized = serde_json::to_string(&event).unwrap();
    let value: Value = serde_json::from_str(&reserialized).unwrap();
    assert_eq!(value["runtime_id"], "s1");
    assert!(
        value.get("session_id").is_none(),
        "session_id field should NOT appear in serialized Cancelled (only runtime_id)"
    );
}

// ===========================================================================
// 向后兼容测试：daemon 旧版本可能缺字段
// ===========================================================================

#[test]
fn tool_result_without_args_preview_deserializes_to_default() {
    // 旧 daemon 可能不发送 args_preview；#[serde(default)] 让它默认为空字符串
    let json = r#"{"type":"tool_result","agent_id":"root","call_id":"c1","tool_name":"bash","output_preview":"ok","is_error":false}"#;
    let event = assert_parses(json);
    match event {
        RuntimeSseEvent::ToolResult { args_preview, .. } => {
            assert_eq!(args_preview, "");
        }
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn tool_call_without_args_preview_and_detail_deserializes_to_defaults() {
    let json = r#"{"type":"tool_call","agent_id":"root","call_id":"c1","tool_name":"bash","status":"running"}"#;
    let event = assert_parses(json);
    match event {
        RuntimeSseEvent::ToolCall {
            args_preview,
            detail,
            ..
        } => {
            assert_eq!(args_preview, "");
            assert_eq!(detail, "");
        }
        _ => panic!("expected ToolCall"),
    }
}

#[test]
fn loop_end_without_summary_fields_deserializes_to_defaults() {
    let json = r#"{"type":"loop_end","agent_id":"root"}"#;
    let event = assert_parses(json);
    match event {
        RuntimeSseEvent::LoopEnd {
            turn_count,
            total_tokens,
            stop_reason,
            ..
        } => {
            assert_eq!(turn_count, 0);
            assert_eq!(total_tokens, 0);
            assert_eq!(stop_reason, "");
        }
        _ => panic!("expected LoopEnd"),
    }
}

#[test]
fn done_without_actions_deserializes_to_empty_vec() {
    let json = r#"{"type":"done","reply":"Hi","raw_reply":"Hi","conversation_id":"conv","runtime_id":"s1","turn_count":1,"total_tokens":100,"prompt_tokens":10,"completion_tokens":5,"estimated_input_tokens":8,"messages":[],"stop_reason":"end_turn"}"#;
    let event = assert_parses(json);
    match event {
        RuntimeSseEvent::Done { actions, .. } => {
            assert!(actions.is_empty());
        }
        _ => panic!("expected Done"),
    }
}

// ===========================================================================
// 向前兼容测试：daemon 新增事件类型/字段
// ===========================================================================

#[test]
fn parse_sse_data_returns_none_for_unknown_event_type() {
    // daemon 未来可能新增事件类型；旧客户端应跳过而非报错
    assert_returns_none(r#"{"type":"some_new_event","foo":"bar"}"#);
    assert_returns_none(r#"{"type":"future_variant","field1":1,"field2":[1,2,3]}"#);
}

#[test]
fn parse_sse_data_returns_none_for_empty_and_null() {
    assert_returns_none("");
    assert_returns_none("   ");
    assert_returns_none("null");
}

#[test]
fn parse_sse_data_tolerates_unknown_fields_in_known_events() {
    // daemon 可能新增字段；旧客户端应忽略未知字段而非报错
    // （deny_unknown_fields 未启用——默认行为就是容忍）
    let json = r#"{"type":"text_delta","agent_id":"root","delta":"Hi","snapshot":"Hi","future_field":"ignored"}"#;
    let event = assert_parses(json);
    match event {
        RuntimeSseEvent::TextDelta { delta, .. } => assert_eq!(delta, "Hi"),
        _ => panic!("expected TextDelta"),
    }
}

#[test]
fn parse_sse_data_errors_on_malformed_json() {
    // 真正的 JSON 语法错误仍报错（不静默吞掉）
    assert!(parse_sse_data(r#"{"type":"text_delta","agent_id":"root","#).is_err());
    assert!(parse_sse_data("not json at all").is_err());
}

#[test]
fn parse_sse_data_errors_on_wrong_field_type() {
    // 字段类型不匹配仍报错（强类型化）
    assert!(parse_sse_data(r#"{"type":"turn_start","agent_id":"root","turn":"not-a-number"}"#).is_err());
}

