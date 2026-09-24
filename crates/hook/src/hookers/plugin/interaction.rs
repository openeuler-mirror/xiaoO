use agent_types::common::HookerId;
use agent_types::hook::HookPointId;
use agent_types::interaction::types::InteractionSource;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use serde::Deserialize;
use serde_json::{json, Value};

/// A plugin command's top-level response: either its final output, or a
/// directive to ask the user a question and re-invoke the command with the
/// answer attached. Shared by every plugin hooker that supports the
/// mid-stream `ask_user` interaction protocol (chat / llm / tool); session
/// hooks are ack-only observers and never produce this shape.
#[derive(Debug)]
pub(crate) enum PluginCommandResponse {
    Final(Value),
    AskUser(AskUserDirective),
}

/// The `ask_user` directive payload: the question to surface and the
/// plugin-defined continuation token echoed back alongside the answer.
#[derive(Debug)]
pub(crate) struct AskUserDirective {
    pub(crate) request: PluginAskUserRequest,
    pub(crate) continuation: Value,
}

/// The question shapes a plugin may ask the user mid-hook, tagged by the
/// `kind` field in the plugin's JSON response.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PluginAskUserRequest {
    Confirm {
        prompt: String,
    },
    TextInput {
        prompt: String,
    },
    Choice {
        prompt: String,
        options: Vec<String>,
        allow_custom_input: bool,
    },
}

/// Parse a plugin command's top-level response. A missing or `"final"`
/// `action` means the output is the hook's result; `"ask_user"` starts the
/// interaction round-trip; anything else is a contract violation.
/// `make_err` lifts failures into the caller's own error type.
pub(crate) fn parse_plugin_command_response<E>(
    id: &HookerId,
    output: Value,
    make_err: &impl Fn(String) -> E,
) -> Result<PluginCommandResponse, E> {
    match output.get("action").and_then(Value::as_str) {
        None | Some("final") => Ok(PluginCommandResponse::Final(output)),
        Some("ask_user") => {
            let request = serde_json::from_value::<PluginAskUserRequest>(
                output.get("request").cloned().ok_or_else(|| {
                    make_err(format!(
                        "plugin hooker '{}' response must contain field 'request'",
                        id.0
                    ))
                })?,
            )
            .map_err(|error| {
                make_err(format!(
                    "plugin hooker '{}' ask_user request is invalid: {}",
                    id.0, error
                ))
            })?;
            let continuation = output.get("continuation").cloned().ok_or_else(|| {
                make_err(format!(
                    "plugin hooker '{}' response must contain field 'continuation'",
                    id.0
                ))
            })?;
            Ok(PluginCommandResponse::AskUser(AskUserDirective {
                request,
                continuation,
            }))
        }
        Some(other) => Err(make_err(format!(
            "plugin hooker '{}' returned unsupported action '{}'",
            id.0, other
        ))),
    }
}

/// Attach the hooker identity to a plugin-asked question so the interaction
/// surface can report which hooker (and hook point) prompted the user, then
/// map the plugin request onto the runtime's interaction request shape.
pub(crate) fn with_hooker_interaction_source(
    id: &HookerId,
    hook_point: &HookPointId,
    request: PluginAskUserRequest,
) -> InteractionRequest {
    let source = Some(InteractionSource::Hooker {
        hooker_name: id.0.clone(),
        hook_point: hook_point.0.clone(),
    });

    match request {
        PluginAskUserRequest::Confirm { prompt } => InteractionRequest::Confirm { prompt, source },
        PluginAskUserRequest::TextInput { prompt } => InteractionRequest::TextInput {
            prompt,
            source,
            is_secret: false, // Default to false for plugin requests
        },
        PluginAskUserRequest::Choice {
            prompt,
            options,
            allow_custom_input,
        } => InteractionRequest::Choice {
            prompt,
            options,
            allow_custom_input,
            source,
        },
    }
}

/// Merge the user's answer into the pending payload: the plugin is
/// re-invoked with its original fields plus an `interaction` block carrying
/// the request, the response, and the plugin's continuation token.
pub(crate) fn build_interaction_followup_payload<E>(
    id: &HookerId,
    payload: Value,
    continuation: Value,
    request: &InteractionRequest,
    response: &InteractionResponse,
    make_err: &impl Fn(String) -> E,
) -> Result<Value, E> {
    let mut payload_map = match payload {
        Value::Object(map) => map,
        _ => {
            return Err(make_err(format!(
                "plugin hooker '{}' follow-up payload must be a JSON object",
                id.0
            )));
        }
    };

    payload_map.insert(
        "interaction".to_string(),
        json!({
            "request": request,
            "response": response,
            "continuation": continuation,
        }),
    );
    Ok(Value::Object(payload_map))
}
