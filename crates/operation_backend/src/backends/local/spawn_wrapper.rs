use serde_json::Value;

/// A generic "spawn wrapper" instruction extracted from a tool call's
/// accumulated plugin `extra` channel. Plugins (registered via plugin.json
/// hook points) may contribute an argv `prefix` that the local backend
/// prepends to the tool subprocess — outermost — and, optionally, a flag
/// asking that mutation-class filesystem operations be externalized into a
/// subprocess spawned the same way.
///
/// The instruction is pure data: the host never learns what the wrapper
/// binary is, and other backends simply ignore it. Unknown keys inside an
/// entry are ignored for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpawnWrapper {
    /// Non-empty argv segment prepended to the spawned command. Elements are
    /// raw argv values — never shell-parsed, no quoting/escaping concerns.
    pub(crate) prefix: Vec<String>,
    /// When `true`, mutation-class filesystem requests (write_bytes,
    /// create_dir_all) are executed by a `xiaoo-backend-cli` subprocess whose
    /// argv is also prefixed. Defaults to `false`.
    pub(crate) externalize_fs_writes: bool,
}

/// Extract the first valid `spawn_wrapper` instruction from the plugin
/// `extra` channel. `extra` is the framework-accumulated array of per-hooker
/// entries (`FinalToolCall.extra` / `ExecRequest.extra`); the first entry
/// carrying a well-formed `spawn_wrapper` wins, later ones are logged and
/// skipped, malformed shapes (non-array/empty prefix, non-string elements)
/// are warned about and ignored. Returns `None` when no instruction is
/// present — the caller must then behave exactly as before (structural
/// "not enabled, zero change" guarantee).
pub(crate) fn spawn_wrapper_from_extra(extra: Option<&Value>) -> Option<SpawnWrapper> {
    let entries = match extra {
        Some(Value::Array(entries)) => entries.as_slice(),
        Some(other) => {
            tracing::warn!(
                value_type = %type_name(other),
                "ignoring spawn_wrapper in unexpected extra channel shape (expected an array of hooker entries)"
            );
            return None;
        }
        None => return None,
    };

    let mut winner: Option<SpawnWrapper> = None;
    for entry in entries {
        let Some(wrapper) = entry.get("spawn_wrapper") else {
            continue;
        };
        match parse_spawn_wrapper(wrapper) {
            Some(parsed) => {
                if winner.is_none() {
                    winner = Some(parsed);
                } else {
                    tracing::debug!(
                        hooker_id = entry
                            .get("hooker_id")
                            .and_then(|id| id.as_str())
                            .unwrap_or("?"),
                        "multiple spawn_wrapper instructions; keeping the first one"
                    );
                }
            }
            None => {
                tracing::warn!(
                    hooker_id = entry.get("hooker_id").and_then(|id| id.as_str()).unwrap_or("?"),
                    "ignoring malformed spawn_wrapper instruction (prefix must be a non-empty array of strings)"
                );
            }
        }
    }
    winner
}

fn parse_spawn_wrapper(wrapper: &Value) -> Option<SpawnWrapper> {
    let prefix_value = wrapper.get("prefix")?;
    let Value::Array(prefix) = prefix_value else {
        return None;
    };
    if prefix.is_empty() {
        return None;
    }
    let mut parsed_prefix = Vec::with_capacity(prefix.len());
    for element in prefix {
        parsed_prefix.push(element.as_str()?.to_string());
    }
    let externalize_fs_writes = wrapper
        .get("externalize_fs_writes")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(SpawnWrapper {
        prefix: parsed_prefix,
        externalize_fs_writes,
    })
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/operation_backend/backends/local/spawn_wrapper_test.rs"]
mod tests;
