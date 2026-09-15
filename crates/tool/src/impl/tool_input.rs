use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

/// Deserialize tool-call input, strict first. On failure, retry once after
/// unwrapping any string field holding a JSON array/object (the model sometimes
/// sends a structured arg as a string); unrecoverable input returns the original
/// strict error unchanged.
pub fn parse_tool_input<T: DeserializeOwned>(value: &Value) -> Result<T, serde_json::Error> {
    match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => Ok(parsed),
        Err(original_err) => match coerce_stringified_json(value) {
            Some(coerced) => serde_json::from_value::<T>(coerced).map_err(|_| original_err),
            None => Err(original_err),
        },
    }
}

fn coerce_stringified_json(value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let mut coerced: Map<String, Value> = object.clone();
    let mut changed = false;
    for slot in coerced.values_mut() {
        if let Value::String(text) = slot {
            if let Ok(inner @ (Value::Array(_) | Value::Object(_))) =
                serde_json::from_str::<Value>(text.trim())
            {
                *slot = inner;
                changed = true;
            }
        }
    }
    if changed {
        Some(Value::Object(coerced))
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/tool/impl/tool_input_test.rs"]
mod tests;
