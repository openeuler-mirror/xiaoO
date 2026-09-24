use super::hooker_visible_call;
use agent_types::tool::FinalToolCall;

#[test]
fn hooker_visible_call_strips_accumulated_extra() {
    let final_call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        input: serde_json::json!({"command": "pwd"}),
        extra: Some(serde_json::json!([
            {"hooker_id": "plugin_a", "file": []}
        ])),
    };

    let visible = hooker_visible_call(&final_call);

    assert!(visible.extra.is_none());
    assert_eq!(visible.call_id, final_call.call_id);
    assert_eq!(visible.input, final_call.input);
    // the source keeps its extra for backend delivery
    assert!(final_call.extra.is_some());
}

#[test]
fn plugin_visible_call_serializes_without_extra_key() {
    let final_call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        input: serde_json::json!({"command": "pwd"}),
        extra: Some(serde_json::json!([
            {"hooker_id": "plugin_a", "file": []}
        ])),
    };

    let visible = hooker_visible_call(&final_call);
    let serialized = serde_json::to_value(&visible).unwrap();

    assert!(
        serialized.get("extra").is_none(),
        "plugin input must not carry an extra key"
    );
    assert_eq!(serialized["call_id"], "call-1");
    assert_eq!(serialized["tool_name"], "bash");

    // the accumulated call still serializes the extra channel for the backend
    let with_extra = serde_json::to_value(&final_call).unwrap();
    assert!(with_extra.get("extra").is_some());
}

#[test]
fn accumulate_extra_injects_hooker_id_and_appends() {
    use super::accumulate_extra;
    use agent_types::common::HookerId;

    let mut call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "file_write".to_string(),
        input: serde_json::json!({"file_path": "a.txt"}),
        ..Default::default()
    };

    // First contribution creates the array with the hooker id injected.
    accumulate_extra(
        &mut call,
        &HookerId("plugin_a".to_string()),
        Some(serde_json::json!({"spawn_wrapper": {"prefix": ["/bin/w"]}})),
    );
    assert_eq!(
        call.extra,
        Some(serde_json::json!([
            {"hooker_id": "plugin_a", "spawn_wrapper": {"prefix": ["/bin/w"]}}
        ]))
    );

    // A second contribution appends instead of replacing.
    accumulate_extra(
        &mut call,
        &HookerId("plugin_b".to_string()),
        Some(serde_json::json!({"note": "hi"})),
    );
    assert_eq!(
        call.extra,
        Some(serde_json::json!([
            {"hooker_id": "plugin_a", "spawn_wrapper": {"prefix": ["/bin/w"]}},
            {"hooker_id": "plugin_b", "note": "hi"}
        ]))
    );

    // None is a no-op (legacy plugins without extra).
    accumulate_extra(&mut call, &HookerId("plugin_c".to_string()), None);
    assert_eq!(
        call.extra,
        Some(serde_json::json!([
            {"hooker_id": "plugin_a", "spawn_wrapper": {"prefix": ["/bin/w"]}},
            {"hooker_id": "plugin_b", "note": "hi"}
        ]))
    );

    // A non-object payload still accumulates (hooker id cannot be injected,
    // but the entry is preserved verbatim inside the entries array).
    let mut bare = FinalToolCall::default();
    accumulate_extra(
        &mut bare,
        &HookerId("plugin_d".to_string()),
        Some(serde_json::json!([1, 2])),
    );
    assert_eq!(bare.extra, Some(serde_json::json!([[1, 2]])));
}
