use crate::wire_types::{WireCompletionTokensDetails, WireToolChoice, WireUsage};
use agent_types::{LlmRequest, ReasoningEffort, Tool};

use super::types::*;

pub(crate) fn to_gemini_function_declaration(tool: &Tool) -> GeminiFunctionDeclaration {
    GeminiFunctionDeclaration {
        name: tool.name.clone(),
        description: Some(tool.description.clone()),
        parameters: Some(sanitize_gemini_schema(
            &tool.parameters,
            GeminiSchemaMode::ToolParameters,
        )),
    }
}

pub(crate) fn to_gemini_tool_config(tool_choice: &WireToolChoice) -> Option<serde_json::Value> {
    match tool_choice {
        WireToolChoice::String(s) => match s.as_str() {
            "none" => Some(serde_json::json!({"functionCallingConfig": {"mode": "NONE"}})),
            "auto" => Some(serde_json::json!({"functionCallingConfig": {"mode": "AUTO"}})),
            "required" => Some(serde_json::json!({"functionCallingConfig": {"mode": "ANY"}})),
            _ => None,
        },
        WireToolChoice::Function { function, .. } => Some(serde_json::json!({
            "functionCallingConfig": { "mode": "ANY", "allowedFunctionNames": [function.name] }
        })),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum GeminiSchemaMode {
    ResponseSchema,
    ToolParameters,
}

pub(crate) fn sanitize_gemini_schema(
    schema: &serde_json::Value,
    mode: GeminiSchemaMode,
) -> serde_json::Value {
    match schema {
        serde_json::Value::Object(map) => {
            let mut converted = serde_json::Map::new();
            for (key, value) in map {
                if matches!(key.as_str(), "additionalProperties" | "$schema" | "$id") {
                    continue;
                }
                let new_value = if key == "type" {
                    match value.as_str() {
                        Some("string") => serde_json::json!("STRING"),
                        Some("number") => serde_json::json!("NUMBER"),
                        Some("integer") => serde_json::json!("INTEGER"),
                        Some("boolean") => serde_json::json!("BOOLEAN"),
                        Some("array") => serde_json::json!("ARRAY"),
                        Some("object") => serde_json::json!("OBJECT"),
                        Some("null") => serde_json::json!("NULL"),
                        _ => value.clone(),
                    }
                } else {
                    sanitize_gemini_schema(value, mode)
                };
                let target_key = match (mode, key.as_str()) {
                    (GeminiSchemaMode::ResponseSchema, "anyOf") => "anyOf".to_string(),
                    _ => key.clone(),
                };
                converted.insert(target_key, new_value);
            }
            serde_json::Value::Object(converted)
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| sanitize_gemini_schema(item, mode))
                .collect(),
        ),
        _ => schema.clone(),
    }
}

pub(crate) fn normalize_model_name(model: &str) -> String {
    if model.starts_with("models/") {
        model.to_string()
    } else {
        format!("models/{}", model)
    }
}

pub(crate) fn build_gemini_request_body(request: &LlmRequest, model: &str) -> GeminiRequestBody {
    let wire_messages = crate::convert::chat_messages_to_wire(&request.messages);
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();

    for msg in &wire_messages {
        if msg.role == "system" {
            if let Some(ref content) = msg.content {
                system_parts.push(content.clone());
            }
        } else {
            let role = match msg.role.as_str() {
                "assistant" => Some("model".to_string()),
                "user" => Some("user".to_string()),
                _ => Some("user".to_string()),
            };

            if let Some(ref tool_calls) = msg.tool_calls {
                for tool_call in tool_calls {
                    contents.push(GeminiContent {
                        role: Some("model".to_string()),
                        parts: vec![GeminiPart::function_call(
                            tool_call.function.name.clone(),
                            parse_tool_arguments(&tool_call.function.arguments),
                        )],
                    });
                }
            }

            if msg.role == "tool" {
                contents.push(GeminiContent {
                    role,
                    parts: vec![GeminiPart::function_response(
                        msg.tool_call_id.clone().unwrap_or_default(),
                        msg.content.clone().unwrap_or_default(),
                    )],
                });
                continue;
            }

            if let Some(ref content) = msg.content {
                contents.push(GeminiContent {
                    role,
                    parts: vec![GeminiPart::text(content.clone())],
                });
            }
        }
    }

    let system_instruction = (!system_parts.is_empty()).then(|| GeminiContent {
        role: None,
        parts: vec![GeminiPart::text(system_parts.join("\n\n"))],
    });

    let wire_format = crate::convert::response_format_to_wire(&request.response_format);
    let (response_mime_type, response_json_schema) = wire_format
        .as_ref()
        .map(|rf| match rf.format_type.as_str() {
            "json_object" => (Some("application/json".to_string()), None),
            "json_schema" => {
                let schema = rf
                    .json_schema
                    .as_ref()
                    .map(|js| sanitize_gemini_schema(&js.schema, GeminiSchemaMode::ResponseSchema));
                (Some("application/json".to_string()), schema)
            }
            _ => (None, None),
        })
        .unwrap_or((None, None));

    let tools = if request.tools.is_empty() {
        None
    } else {
        Some(vec![GeminiTool {
            function_declarations: request
                .tools
                .iter()
                .map(to_gemini_function_declaration)
                .collect(),
        }])
    };

    let wire_tool_choice = crate::convert::tool_choice_to_wire(&request.tool_choice);
    let tool_config = to_gemini_tool_config(&wire_tool_choice);

    GeminiRequestBody {
        contents,
        system_instruction,
        generation_config: GeminiGenerationConfig {
            max_output_tokens: request.max_tokens.map(|t| t as u32),
            temperature: request.temperature.map(|t| t as f32),
            response_mime_type,
            response_json_schema,
            thinking_config: gemini_thinking_budget(model, request.reasoning_effort)
                .map(|thinking_budget| GeminiThinkingConfig { thinking_budget }),
        },
        tools,
        tool_config,
    }
}

/// Gemini `thinkingBudget` per effort tier. Pro-tier models (2.5-pro,
/// 3-pro …) enforce a positive minimum budget and reject
/// `thinkingBudget: 0` with a 400, so `off` must omit the whole
/// thinking config there (provider default) instead of hard-failing the
/// request. Only models whitelisted by [`supports_disabling_thinking`]
/// send the zero budget; anything unrecognized stays on the safe side
/// and omits.
fn gemini_thinking_budget(model: &str, effort: ReasoningEffort) -> Option<i32> {
    match effort {
        ReasoningEffort::Default => None,
        ReasoningEffort::Off => supports_disabling_thinking(model).then_some(0),
        ReasoningEffort::High => Some(8192),
        ReasoningEffort::Max => Some(24576),
    }
}

/// Whether the model is verified to accept an explicit
/// `thinkingBudget: 0`. Deliberately a whitelist: acceptance is
/// per-family (the pro tier rejects the zero budget outright, and a
/// future flash variant may too), so a model is only trusted to take
/// the explicit disable once confirmed against the API — an unlisted
/// model must degrade to the provider default rather than risk a 400.
/// A family joins this list via prefix match, which also covers its
/// -lite / -latest / -preview-NN suffixes; `gemini-flash-latest` and
/// `-lite-latest` are the rolling aliases that always resolve into the
/// 2.5/3 flash families.
fn supports_disabling_thinking(model: &str) -> bool {
    let leaf = model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    leaf.starts_with("gemini-2.5-flash")
        || leaf.starts_with("gemini-3-flash")
        || leaf.starts_with("gemini-flash-latest")
        || leaf.starts_with("gemini-flash-lite-latest")
}

#[inline]
fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| serde_json::json!({ "raw": arguments }))
}

pub(crate) fn extract_gemini_text(parts: &[GeminiResponsePart]) -> Option<String> {
    let text: String = parts
        .iter()
        .filter_map(|p| p.text.as_ref())
        .cloned()
        .collect();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub(crate) fn extract_gemini_tool_calls(
    parts: &[GeminiResponsePart],
) -> Option<Vec<crate::wire_types::WireToolCall>> {
    let tool_calls: Vec<_> = parts
        .iter()
        .filter_map(|p| p.function_call.as_ref())
        .enumerate()
        .map(|(index, call)| crate::wire_types::WireToolCall {
            id: format!("gemini-call-{}", index),
            call_type: "function".to_string(),
            function: crate::wire_types::WireToolCallFunction {
                name: call.name.clone(),
                arguments: call.args.to_string(),
            },
        })
        .collect();
    if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    }
}

pub(crate) fn extract_gemini_tool_call_deltas(
    parts: &[GeminiResponsePart],
) -> Option<Vec<crate::wire_types::WireToolCallDelta>> {
    let deltas: Vec<_> = parts
        .iter()
        .filter_map(|p| p.function_call.as_ref())
        .enumerate()
        .map(|(index, call)| crate::wire_types::WireToolCallDelta {
            index: index as u32,
            id: None,
            call_type: Some("function".to_string()),
            function: Some(crate::wire_types::WireToolCallFunctionDelta {
                name: Some(call.name.clone()),
                arguments: Some(call.args.to_string()),
            }),
        })
        .collect();
    if deltas.is_empty() {
        None
    } else {
        Some(deltas)
    }
}

/// Normalizes Gemini usage counts into the OpenAI-compatible wire shape
/// assumed by the rest of the pipeline.
///
/// Gemini reports visible output (`candidatesTokenCount`) and thinking
/// (`thoughtsTokenCount`) as disjoint counts, with
/// `totalTokenCount ≈ prompt + candidates + thoughts`. The internal
/// contract instead requires `completion_tokens` to INCLUDE reasoning
/// tokens (OpenAI semantics), so thoughts are folded into completion and
/// carried separately in the details; otherwise downstream
/// `visible = completion - reasoning` accounting would subtract thinking
/// tokens that were never part of the candidate count.
pub(crate) fn gemini_usage_to_wire(u: &GeminiUsageMetadata) -> WireUsage {
    let candidates = u.candidates_token_count.unwrap_or(0);
    let thoughts = u.thoughts_token_count.unwrap_or(0);
    let prompt = u.prompt_token_count.unwrap_or(0);
    WireUsage {
        prompt_tokens: prompt,
        completion_tokens: candidates.saturating_add(thoughts),
        total_tokens: u
            .total_token_count
            .unwrap_or_else(|| prompt.saturating_add(candidates).saturating_add(thoughts)),
        prompt_tokens_details: None,
        completion_tokens_details: Some(WireCompletionTokensDetails {
            reasoning_tokens: thoughts,
        }),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/llm-client/providers/gemini/convert_test.rs"]
mod tests;
