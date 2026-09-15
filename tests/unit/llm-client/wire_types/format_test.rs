use super::*;

#[test]
fn test_response_format_serialization() {
    let format = WireResponseFormat::json_object();
    let json = serde_json::to_string(&format).unwrap();
    assert_eq!(json, r#"{"type":"json_object"}"#);

    let format = WireResponseFormat::text();
    let json = serde_json::to_string(&format).unwrap();
    assert_eq!(json, r#"{"type":"text"}"#);

    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        }
    });
    let format = WireResponseFormat::json_schema("person".to_string(), schema);
    let json = serde_json::to_string(&format).unwrap();
    assert!(json.contains(r#""type":"json_schema""#));
    assert!(json.contains(r#""name":"person""#));
    assert!(json.contains(r#""strict":true"#));
}
