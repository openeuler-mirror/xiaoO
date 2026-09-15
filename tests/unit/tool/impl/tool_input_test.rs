use super::*;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Item {
    content: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Input {
    items: Vec<Item>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Settings {
    mode: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Config {
    settings: Settings,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct Labeled {
    label: String,
}

#[test]
fn strict_input_parses_unchanged() {
    let value = serde_json::json!({ "items": [{ "content": "a" }] });
    let parsed: Input = parse_tool_input(&value).expect("strict input should parse");
    assert_eq!(
        parsed.items,
        vec![Item {
            content: "a".into()
        }]
    );
}

#[test]
fn recovers_stringified_array_argument() {
    let value = serde_json::json!({ "items": "[{\"content\": \"a\"}]" });
    let parsed: Input = parse_tool_input(&value).expect("stringified array should be recovered");
    assert_eq!(
        parsed.items,
        vec![Item {
            content: "a".into()
        }]
    );
}

#[test]
fn recovers_stringified_object_argument() {
    let value = serde_json::json!({ "settings": "{\"mode\": \"fast\"}" });
    let parsed: Config = parse_tool_input(&value).expect("stringified object should be recovered");
    assert_eq!(
        parsed.settings,
        Settings {
            mode: "fast".into()
        }
    );
}

#[test]
fn malformed_inner_json_keeps_original_error() {
    let value = serde_json::json!({ "items": "[{\"content\": }]" });
    let result: Result<Input, _> = parse_tool_input(&value);
    assert!(result.is_err());
}

#[test]
fn json_looking_string_field_is_preserved() {
    let value = serde_json::json!({ "label": "[1, 2, 3]" });
    let parsed: Labeled = parse_tool_input(&value).expect("string field should parse strictly");
    assert_eq!(parsed.label, "[1, 2, 3]");
}

#[test]
fn non_object_input_preserves_original_error() {
    let value = serde_json::json!([1, 2, 3]);
    let result: Result<Input, _> = parse_tool_input(&value);
    assert!(result.is_err());
}
