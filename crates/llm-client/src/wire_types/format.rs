use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct WireResponseFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<JsonSchemaDef>,
}

#[allow(dead_code)]
impl WireResponseFormat {
    pub(crate) fn json_object() -> Self {
        Self {
            format_type: "json_object".to_string(),
            json_schema: None,
        }
    }

    pub(crate) fn json_schema(name: String, schema: serde_json::Value) -> Self {
        Self {
            format_type: "json_schema".to_string(),
            json_schema: Some(JsonSchemaDef {
                name,
                strict: Some(true),
                schema,
            }),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn text() -> Self {
        Self {
            format_type: "text".to_string(),
            json_schema: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct JsonSchemaDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    pub schema: serde_json::Value,
}

#[cfg(test)]
mod tests {
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
}
