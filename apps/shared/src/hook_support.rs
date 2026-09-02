use agent_types::chat::{
    ChatMessageHookInput, ChatSystemTransformInput, CommandExecuteBeforeInput, ModelRef,
};
use agent_types::common::{BuildError, HookerId};
use agent_types::hook::{
    HookInvokeInput, HookInvokeMetadata, HookInvokePrimary, HookerDefaultMode, HookerRegistryConfig,
};
use agent_types::llm::{
    AssistantMessage, ChatMessage, ContentBlock, LlmError, LlmRequest, LlmResponse, MessageRole,
    ResponseFormat, StopReason, ToolChoice, Usage,
};
use agent_types::session::{
    SessionClosedHookInput, SessionCreatedHookInput, SessionStateHookInput,
};
use agent_types::tool::{
    ErrorToolHookInput, FinalToolCall, PostToolHookInput, PreToolHookInput, RawToolOutcome,
    ToolExecutionError,
};
use hook::framework::HookerRegistryBuilderImpl;
use hook::{resolve_hook_point_category, HookPointCategory, HookerRegistryBuilder};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HookCatalogReport {
    pub schema_version: u32,
    pub default_mode: &'static str,
    pub max_prompt_chain_depth: usize,
    pub plugins: Vec<HookPluginSummary>,
    pub hooks: Vec<HookSummary>,
    pub recent_executions: Vec<trace::HookExecutionDiagnosticSummary>,
    pub recent_error_kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HookPluginSummary {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Serialize)]
pub struct HookSummary {
    pub id: String,
    pub hook_point: String,
    pub category: &'static str,
    pub enabled: bool,
    pub has_policy: bool,
}

#[derive(Debug, Serialize)]
pub struct HookTestReport {
    pub schema_version: u32,
    pub hooker_id: String,
    pub hook_point: String,
    pub category: &'static str,
    pub enabled: bool,
    pub success: bool,
    pub duration_ms: u128,
    pub output_kind: Option<&'static str>,
    pub action_count: usize,
    pub error_kind: Option<&'static str>,
}

pub fn hook_catalog(config: &HookerRegistryConfig) -> Result<HookCatalogReport, BuildError> {
    let registry = HookerRegistryBuilderImpl::new()
        .with_config(config.clone())
        .build()?;
    let mut hooks = registry
        .list()
        .into_iter()
        .map(|hooker| {
            let id = hooker.id();
            Ok(HookSummary {
                id: id.to_string(),
                hook_point: hooker.hook_point().0.clone(),
                category: category_name(resolve_hook_point_category(hooker.hook_point())?),
                enabled: registry.is_enabled(id),
                has_policy: registry.policy_for(id).is_some(),
            })
        })
        .collect::<Result<Vec<_>, BuildError>>()?;
    hooks.sort_by(|left, right| left.id.cmp(&right.id));

    Ok(HookCatalogReport {
        schema_version: 1,
        default_mode: match config.default {
            HookerDefaultMode::All => "all",
            HookerDefaultMode::None => "none",
        },
        max_prompt_chain_depth: config.max_prompt_chain_depth,
        plugins: config
            .plugins
            .iter()
            .map(|path| HookPluginSummary {
                path: path.clone(),
                exists: std::path::Path::new(path).is_file(),
            })
            .collect(),
        hooks,
        recent_executions: Vec::new(),
        recent_error_kind: None,
    })
}

pub async fn test_hook(
    config: &HookerRegistryConfig,
    hooker_id: &str,
) -> Result<HookTestReport, BuildError> {
    let registry = HookerRegistryBuilderImpl::new()
        .with_config(config.clone())
        .build()?;
    let id = HookerId(hooker_id.to_string());
    let hooker = registry.get(&id).ok_or_else(|| BuildError::InvalidConfig {
        message: format!("hooker `{hooker_id}` is not registered"),
    })?;
    let category = resolve_hook_point_category(hooker.hook_point())?;
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(35),
        hooker.invoke(
            synthetic_input(category),
            &xiaoo_api::runtime::NoopRuntimeView::new(),
        ),
    )
    .await;
    let (success, output_kind, action_count, error_kind) = match result {
        Ok(Ok(output)) => (
            true,
            Some(output_kind(&output.primary)),
            output.actions.len(),
            None,
        ),
        Ok(Err(_)) => (false, None, 0, Some("invoke_failed")),
        Err(_) => (false, None, 0, Some("timeout")),
    };
    Ok(HookTestReport {
        schema_version: 1,
        hooker_id: hooker.id().to_string(),
        hook_point: hooker.hook_point().0.clone(),
        category: category_name(category),
        enabled: registry.is_enabled(&id),
        success,
        duration_ms: started.elapsed().as_millis(),
        output_kind,
        action_count,
        error_kind,
    })
}

fn synthetic_input(category: HookPointCategory) -> HookInvokeInput {
    let metadata = HookInvokeMetadata::default();
    let call = FinalToolCall {
        call_id: "xiaoo-hook-test".to_string(),
        tool_name: "hook_test".to_string(),
        input: serde_json::json!({}),
        extra: None,
    };
    let request = LlmRequest {
        messages: vec![test_chat_message()],
        tools: Vec::new(),
        tool_choice: ToolChoice::None,
        max_tokens: Some(1),
        temperature: Some(0.0),
        response_format: ResponseFormat::Text,
        reasoning_effort: Default::default(),
    };
    match category {
        HookPointCategory::ToolPre => HookInvokeInput::Pre {
            input: PreToolHookInput { call },
            metadata,
        },
        HookPointCategory::ToolPost => HookInvokeInput::Post {
            input: PostToolHookInput {
                call,
                outcome: RawToolOutcome::Success {
                    output: String::new(),
                },
            },
            metadata,
        },
        HookPointCategory::ToolError => HookInvokeInput::Error {
            input: ErrorToolHookInput {
                call,
                error: ToolExecutionError::ExecutionFailed {
                    message: "synthetic hook test".to_string(),
                },
            },
            metadata,
        },
        HookPointCategory::LlmPre => HookInvokeInput::LlmPre {
            input: agent_types::llm::PreLlmHookInput { request },
            metadata,
        },
        HookPointCategory::LlmPost => HookInvokeInput::LlmPost {
            input: agent_types::llm::PostLlmHookInput {
                request,
                response: test_llm_response(),
            },
            metadata,
        },
        HookPointCategory::LlmError => HookInvokeInput::LlmError {
            input: agent_types::llm::ErrorLlmHookInput {
                request,
                error: LlmError::RequestFailed {
                    message: "synthetic hook test".to_string(),
                },
            },
            metadata,
        },
        HookPointCategory::SessionCreated => HookInvokeInput::SessionCreated {
            input: SessionCreatedHookInput {
                session_id: "xiaoo-hook-test".to_string(),
                sender_id: "xiaoo-vscode".to_string(),
            },
            metadata,
        },
        HookPointCategory::SessionClosed => HookInvokeInput::SessionClosed {
            input: SessionClosedHookInput {
                session_id: "xiaoo-hook-test".to_string(),
                sender_id: "xiaoo-vscode".to_string(),
            },
            metadata,
        },
        HookPointCategory::SessionState => HookInvokeInput::SessionState {
            input: SessionStateHookInput {
                session_id: "xiaoo-hook-test".to_string(),
                sender_id: "xiaoo-vscode".to_string(),
                agent_id: "xiaoo-hook-test".to_string(),
                state: "idle".to_string(),
                outcome: "complete".to_string(),
            },
            metadata,
        },
        HookPointCategory::ChatSystemTransform => HookInvokeInput::ChatSystemTransform {
            input: ChatSystemTransformInput {
                session_id: Some("xiaoo-hook-test".to_string()),
                model: ModelRef::default(),
                current_system: vec!["Synthetic hook test".to_string()],
            },
            metadata,
        },
        HookPointCategory::ChatMessage => HookInvokeInput::ChatMessage {
            input: ChatMessageHookInput {
                session_id: "xiaoo-hook-test".to_string(),
                agent: None,
                model: None,
                message_id: None,
                message: test_chat_message(),
                prior_message_count: 0,
            },
            metadata,
        },
        HookPointCategory::CommandExecuteBefore => HookInvokeInput::CommandExecuteBefore {
            input: CommandExecuteBeforeInput {
                command: "hook-test".to_string(),
                session_id: "xiaoo-hook-test".to_string(),
                arguments: String::new(),
                body: "Synthetic hook test".to_string(),
            },
            metadata,
        },
    }
}

fn test_chat_message() -> ChatMessage {
    ChatMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: "Synthetic hook test".to_string(),
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    }
}

fn test_llm_response() -> LlmResponse {
    LlmResponse {
        message: AssistantMessage {
            text: Some(String::new()),
            reasoning_content: None,
            tool_calls: Vec::new(),
            usage: Usage::default(),
            stop_reason: StopReason::EndTurn,
        },
        kv_cache_chunk_hashes: Vec::new(),
    }
}

fn output_kind(output: &HookInvokePrimary) -> &'static str {
    match output {
        HookInvokePrimary::Pre(_) => "tool_pre",
        HookInvokePrimary::Post(_) => "tool_post",
        HookInvokePrimary::Error(_) => "tool_error",
        HookInvokePrimary::LlmPre(_) => "llm_pre",
        HookInvokePrimary::LlmPost(_) => "llm_post",
        HookInvokePrimary::LlmError(_) => "llm_error",
        HookInvokePrimary::SessionCreated(_) => "session_created",
        HookInvokePrimary::SessionClosed(_) => "session_closed",
        HookInvokePrimary::SessionState(_) => "session_state",
        HookInvokePrimary::ChatSystemTransform(_) => "chat_system_transform",
        HookInvokePrimary::ChatMessage(_) => "chat_message",
        HookInvokePrimary::CommandExecuteBefore(_) => "command_execute_before",
    }
}

fn category_name(category: HookPointCategory) -> &'static str {
    match category {
        HookPointCategory::ToolPre => "tool_pre",
        HookPointCategory::ToolPost => "tool_post",
        HookPointCategory::ToolError => "tool_error",
        HookPointCategory::LlmPre => "llm_pre",
        HookPointCategory::LlmPost => "llm_post",
        HookPointCategory::LlmError => "llm_error",
        HookPointCategory::SessionCreated => "session_created",
        HookPointCategory::SessionClosed => "session_closed",
        HookPointCategory::SessionState => "session_state",
        HookPointCategory::ChatSystemTransform => "chat_system_transform",
        HookPointCategory::ChatMessage => "chat_message",
        HookPointCategory::CommandExecuteBefore => "command_execute_before",
    }
}

#[cfg(test)]
mod tests {
    use super::{hook_catalog, test_hook};
    use agent_types::common::HookerId;
    use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};

    #[test]
    fn reports_registered_hooks_and_effective_state() {
        let mut config = HookerRegistryConfig {
            default: HookerDefaultMode::None,
            ..HookerRegistryConfig::default()
        };
        config
            .enabled
            .push(HookerId("builtin_session_created_hooker".to_string()));
        config.policies.insert(
            HookerId("builtin_session_created_hooker".to_string()),
            serde_json::json!({ "message": "ready" }),
        );

        let report = hook_catalog(&config).expect("hook catalog");
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.default_mode, "none");
        let hook = report
            .hooks
            .iter()
            .find(|hook| hook.id == "builtin_session_created_hooker")
            .expect("session hook");
        assert!(hook.enabled);
        assert!(hook.has_policy);
        assert_eq!(hook.category, "session_created");
    }

    #[tokio::test]
    async fn tests_registered_hook_with_synthetic_runtime_input() {
        let config = HookerRegistryConfig::default();
        let report = test_hook(&config, "builtin_session_created_hooker")
            .await
            .expect("hook test");

        assert!(report.success);
        assert_eq!(report.category, "session_created");
        assert_eq!(report.output_kind, Some("session_created"));
        assert_eq!(report.action_count, 0);
        assert!(report.error_kind.is_none());
    }
}
