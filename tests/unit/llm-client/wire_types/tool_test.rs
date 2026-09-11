use super::*;

#[test]
fn test_tool_serialization() {
    let params = serde_json::json!({
        "type": "object",
        "properties": {
            "location": {"type": "string"}
        }
    });
    let tool = WireTool::function(
        "get_weather".to_string(),
        "Get weather for a location".to_string(),
        params,
    );

    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains(r#""type":"function""#));
    assert!(json.contains(r#""name":"get_weather""#));
    assert!(json.contains(r#""description":"Get weather for a location""#));
}

#[test]
fn test_tool_choice_serialization() {
    let choice = WireToolChoice::auto();
    let json = serde_json::to_string(&choice).unwrap();
    assert_eq!(json, r#""auto""#);

    let choice = WireToolChoice::none();
    let json = serde_json::to_string(&choice).unwrap();
    assert_eq!(json, r#""none""#);

    let choice = WireToolChoice::required();
    let json = serde_json::to_string(&choice).unwrap();
    assert_eq!(json, r#""required""#);

    let choice = WireToolChoice::function("get_weather".to_string());
    let json = serde_json::to_string(&choice).unwrap();
    assert!(json.contains(r#""type":"function""#));
    assert!(json.contains(r#""name":"get_weather""#));
}

#[test]
fn test_tool_call_serialization() {
    let tool_call = WireToolCall {
        id: "call_abc123".to_string(),
        call_type: "function".to_string(),
        function: WireToolCallFunction {
            name: "get_weather".to_string(),
            arguments: r#"{"location":"San Francisco"}"#.to_string(),
        },
    };

    let json = serde_json::to_string(&tool_call).unwrap();
    assert!(json.contains(r#""id":"call_abc123""#));
    assert!(json.contains(r#""type":"function""#));
    assert!(json.contains(r#""name":"get_weather""#));
}

#[test]
fn test_tool_call_delta_serialization() {
    let delta = WireToolCallDelta {
        index: 0,
        id: Some("call_abc123".to_string()),
        call_type: Some("function".to_string()),
        function: Some(WireToolCallFunctionDelta {
            name: Some("get_weather".to_string()),
            arguments: Some(r#"{"loc"#.to_string()),
        }),
    };

    let json = serde_json::to_string(&delta).unwrap();
    assert!(json.contains(r#""index":0"#));
    assert!(json.contains(r#""id":"call_abc123""#));
}
