use super::*;

#[test]
fn test_schema_to_format_description_simple() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "age": {"type": "integer"}
        },
        "required": ["name", "age"]
    });
    let desc = ZhipuProvider::schema_to_format_description(&schema);
    assert!(desc.contains("\"name\": string"));
    assert!(desc.contains("\"age\": integer"));
    assert!(desc.contains("Required fields: name, age"));
}

#[test]
fn test_schema_to_format_description_with_description() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {"type": "string", "description": "The person's name"},
            "age": {"type": "integer", "description": "The person's age"}
        }
    });
    let desc = ZhipuProvider::schema_to_format_description(&schema);
    assert!(desc.contains("// The person's name"));
    assert!(desc.contains("// The person's age"));
}
