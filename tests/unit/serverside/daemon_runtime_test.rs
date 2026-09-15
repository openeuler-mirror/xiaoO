use super::{
    build_system_prompt, build_token_budget, force_e2b_remote_roots, resolve_agent_role,
    resolve_allowed_tool_names, resolve_local_workspace, resolve_profile_agent_role,
    resolve_profile_allowed_tool_names, validate_existing_e2b_binding, ConfiguredRuntimeResolver,
    RuntimeCapabilityProfile, MCP_CHATBOT_TOOLS,
};
use crate::daemon_config::{AgentRoleConfig, DaemonConfig};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use tempfile::{tempdir, TempDir};
use xiaoo_api::chat::{AgentId, ReasoningEffort, ToolName};
use xiaoo_shared::backend::GatewayBackendConfig;
use xiaoo_shared::gateway::{
    GatewayEntryContext, LlmRuntimeConfig, SessionRuntimeBuildInput, SessionRuntimeResolveError,
    SessionRuntimeResolver,
};

#[test]
fn token_budget_caps_output_to_preserve_prompt_budget() {
    let budget = build_token_budget(128_000, 150_000);
    assert_eq!(budget.total_budget, 128_000);
    assert_eq!(budget.reserved_for_system, 2_048);
}

#[test]
fn token_budget_with_explicit_window() {
    let budget = build_token_budget(65536, 8192);
    assert_eq!(budget.total_budget, 65536);
    assert_eq!(budget.reserved_for_output, 8192);
    assert_eq!(budget.reserved_for_system, 2048);
}

#[test]
fn build_system_prompt_includes_workspace_agents_before_channel_rules() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("AGENTS.md"), "repo rules").unwrap();
    let request = SessionRuntimeBuildInput {
        session_id: "session".to_string(),
        conversation_id: "conversation".to_string(),
        sender_id: "sender".to_string(),
        channel: Some("feishu".to_string()),
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext::channel(None),
        agent_id_override: None,
        max_turns_override: None,
        subagent_role_id: None,
        llm: None,
        workspace: None,
        skills: None,
    };

    let prompt = build_system_prompt(
        "base rules",
        Some(temp.path()),
        &request,
        &BTreeMap::new(),
        false,
        &Vec::new(),
    );

    assert!(prompt.contains("base rules"));
    assert!(prompt.contains("repo rules"));
    assert!(prompt.contains("当前通道"));
    assert!(prompt.find("repo rules").unwrap() < prompt.find("## 当前通道").unwrap());
}

#[test]
fn resolve_allowed_tool_names_requires_exact_tool_names() {
    let all_tool_names = vec![
        ToolName("file_edit".to_string()),
        ToolName("file_write".to_string()),
    ];
    let agent_role = AgentRoleConfig {
        description: String::new(),
        prompt: None,
        max_turns: None,
        tools: BTreeMap::from([
            ("write".to_string(), false),
            ("file_write".to_string(), false),
        ]),
    };

    let allowed = resolve_allowed_tool_names(&all_tool_names, Some(&agent_role));
    let allowed: Vec<_> = allowed.into_iter().map(|tool| tool.0).collect();

    assert!(allowed.contains(&"file_edit".to_string()));
    assert!(!allowed.contains(&"file_write".to_string()));
}

#[test]
fn mcp_profiles_apply_strict_tool_boundaries() {
    let all = [
        "web_search",
        "webfetch",
        "file_read",
        "glob",
        "grep",
        "file_write",
        "bash",
        "skill",
        "spawn_subagent",
        "ask_user_question",
        "send_file",
        "plugin_custom",
    ]
    .into_iter()
    .map(|name| ToolName(name.to_string()))
    .collect::<Vec<_>>();

    let chatbot =
        resolve_profile_allowed_tool_names(&all, RuntimeCapabilityProfile::McpChatbot, None);
    let chatbot = chatbot.into_iter().map(|name| name.0).collect::<Vec<_>>();
    assert_eq!(chatbot, MCP_CHATBOT_TOOLS.map(str::to_string));

    let agent = resolve_profile_allowed_tool_names(&all, RuntimeCapabilityProfile::McpAgent, None);
    let agent = agent.into_iter().map(|name| name.0).collect::<Vec<_>>();
    assert!(agent.contains(&"file_write".to_string()));
    assert!(agent.contains(&"spawn_subagent".to_string()));
    assert!(agent.contains(&"plugin_custom".to_string()));
    assert!(!agent.contains(&"ask_user_question".to_string()));
    assert!(!agent.contains(&"send_file".to_string()));

    let restricted_role = AgentRoleConfig {
        description: String::new(),
        prompt: None,
        max_turns: None,
        tools: BTreeMap::from([("spawn_subagent".to_string(), false)]),
    };
    let restricted = resolve_profile_allowed_tool_names(
        &all,
        RuntimeCapabilityProfile::McpAgent,
        Some(&restricted_role),
    )
    .into_iter()
    .map(|name| name.0)
    .collect::<Vec<_>>();
    assert!(!restricted.contains(&"spawn_subagent".to_string()));
    assert!(restricted.contains(&"file_write".to_string()));
    assert!(!restricted.contains(&"ask_user_question".to_string()));
}

#[test]
fn local_runtime_uses_request_workspace() {
    let default_workspace = tempdir().expect("default workspace");
    let requested_workspace = tempdir().expect("requested workspace");
    let request = SessionRuntimeBuildInput {
        session_id: "mcp-agent-session".to_string(),
        conversation_id: "conversation".to_string(),
        sender_id: "sender".to_string(),
        channel: None,
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext {
            kind: Some(xiaoo_shared::gateway::GatewayEntryKind::Mcp),
            instance_id: Some("agent".to_string()),
            runtime_profile_id: None,
            build_tags: Vec::new(),
        },
        agent_id_override: None,
        max_turns_override: None,
        subagent_role_id: None,
        llm: None,
        workspace: Some(requested_workspace.path().to_path_buf()),
        skills: None,
    };

    let resolved = resolve_local_workspace(&request, None, default_workspace.path())
        .expect("local workspace should resolve");
    assert_eq!(
        resolved,
        requested_workspace
            .path()
            .canonicalize()
            .expect("canonical workspace")
    );
}

#[test]
fn resolve_agent_role_uses_runtime_profile_id() {
    let mut agent_roles = BTreeMap::new();
    agent_roles.insert(
        "code-reviewer".to_string(),
        AgentRoleConfig {
            description: "Reviews code".to_string(),
            prompt: Some("You are a code reviewer.".to_string()),
            max_turns: None,
            tools: BTreeMap::new(),
        },
    );
    let request = SessionRuntimeBuildInput {
        session_id: "session".to_string(),
        conversation_id: "conversation".to_string(),
        sender_id: "sender".to_string(),
        channel: Some("http".to_string()),
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext {
            runtime_profile_id: Some("code-reviewer".to_string()),
            ..GatewayEntryContext::channel(None)
        },
        agent_id_override: None,
        max_turns_override: None,
        subagent_role_id: None,
        llm: None,
        workspace: None,
        skills: None,
    };

    let resolved = resolve_agent_role(&agent_roles, &request)
        .expect("agent role should resolve")
        .expect("agent role should exist");
    assert_eq!(resolved.prompt.as_deref(), Some("You are a code reviewer."));
}

#[test]
fn mcp_agent_role_applies_only_to_the_root_lane() {
    let mut agent_roles = BTreeMap::new();
    agent_roles.insert(
        "xuanyuan".to_string(),
        AgentRoleConfig {
            description: "Operations controller".to_string(),
            prompt: Some("Run the delegated operations workflow.".to_string()),
            max_turns: Some(24),
            tools: BTreeMap::new(),
        },
    );
    let request = SessionRuntimeBuildInput {
        session_id: "mcp-agent-session".to_string(),
        conversation_id: "conversation".to_string(),
        sender_id: "mcp-user".to_string(),
        channel: None,
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext {
            kind: Some(xiaoo_shared::gateway::GatewayEntryKind::Mcp),
            instance_id: Some("agent".to_string()),
            runtime_profile_id: Some("xuanyuan".to_string()),
            build_tags: Vec::new(),
        },
        agent_id_override: None,
        max_turns_override: None,
        subagent_role_id: None,
        llm: None,
        workspace: None,
        skills: None,
    };

    let root_role = resolve_profile_agent_role(
        &agent_roles,
        &request,
        RuntimeCapabilityProfile::McpAgent,
        false,
    )
    .expect("MCP root role should resolve")
    .expect("MCP root should use the configured role");
    assert_eq!(
        root_role.prompt.as_deref(),
        Some("Run the delegated operations workflow.")
    );

    let child_role = resolve_profile_agent_role(
        &agent_roles,
        &request,
        RuntimeCapabilityProfile::McpAgent,
        true,
    )
    .expect("MCP child role resolution should succeed");
    assert!(
        child_role.is_none(),
        "subagent lanes must not inherit the root Xuanyuan prompt"
    );
}

#[test]
fn existing_e2b_binding_inherits_omissions_and_rejects_changes() {
    let workspace = PathBuf::from("/host/workspace");
    let skill_root = PathBuf::from("/host/skills");
    let binding = xiaoo_shared::gateway::RuntimeBootstrapBinding {
        source_workspace: Some(workspace.clone()),
        source_skill_roots: vec![skill_root.clone()],
        content_digest: "digest".to_string(),
        remote_workspace_root: PathBuf::from("/home/user/workspace"),
        remote_skill_roots: vec![PathBuf::from("/home/user/.xiaoo/skills/0")],
        skills: Vec::new(),
        manifest_version: xiaoo_shared::gateway::E2B_BOOTSTRAP_MANIFEST_VERSION,
    };
    let mut request = SessionRuntimeBuildInput {
        session_id: "runtime-1".to_string(),
        conversation_id: "conversation".to_string(),
        sender_id: "sender".to_string(),
        channel: None,
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext::default(),
        agent_id_override: None,
        max_turns_override: None,
        subagent_role_id: None,
        llm: None,
        workspace: None,
        skills: None,
    };

    validate_existing_e2b_binding(&request, None, None, &binding)
        .expect("omission inherits binding");

    request.workspace = Some(workspace.clone());
    request.skills = Some(vec![skill_root.clone()]);
    validate_existing_e2b_binding(
        &request,
        Some(&workspace),
        Some(&vec![skill_root]),
        &binding,
    )
    .expect("same canonical binding is accepted");

    let different = PathBuf::from("/host/different");
    assert!(matches!(
        validate_existing_e2b_binding(
            &request,
            Some(&different),
            Some(&binding.source_skill_roots),
            &binding,
        ),
        Err(SessionRuntimeResolveError::BootstrapConflict { .. })
    ));

    request.workspace = None;
    request.skills = Some(Vec::new());
    assert!(matches!(
        validate_existing_e2b_binding(&request, None, Some(&Vec::new()), &binding),
        Err(SessionRuntimeResolveError::BootstrapConflict { .. })
    ));
}

#[test]
fn e2b_api_runtime_forces_fixed_remote_roots() {
    let backend = force_e2b_remote_roots(GatewayBackendConfig::new(
        "e2b",
        serde_json::json!({
            "workspaceRoot": "/configured/workspace",
            "homeDir": "/configured/home",
            "api_key_env": "E2B_API_KEY"
        }),
    ));

    assert_eq!(backend.options["workspace_root"], "/home/user/workspace");
    assert_eq!(backend.options["home_dir"], "/home/user");
    assert!(backend.options.get("workspaceRoot").is_none());
    assert!(backend.options.get("homeDir").is_none());
    assert_eq!(backend.options["api_key_env"], "E2B_API_KEY");
}

/// Pins that when `agent_id_override` is set, the resolved
/// `descriptor.agent_id` equals the override (so
/// `SharedToolEventSink` scopes subagent tool-lifecycle events
/// with the subagent's id, not the root's), and the tool registry
/// exposes visible tools for the subagent's agent_id.
#[tokio::test]
async fn resolve_honors_agent_id_override_for_subagent_lanes() {
    let (config, _workspace_temp) = minimal_test_config().await;
    let resolver = ConfiguredRuntimeResolver::from_config(&config)
        .await
        .expect("construct resolver from minimal config");

    let subagent_agent_id = AgentId(uuid::Uuid::new_v4().to_string());

    let request = SessionRuntimeBuildInput {
        session_id: "test-session".to_string(),
        conversation_id: "test-session".to_string(),
        sender_id: "test-user".to_string(),
        channel: None,
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext::tui(None),
        agent_id_override: Some(subagent_agent_id.clone()),
        max_turns_override: None,
        subagent_role_id: None,
        llm: None,
        workspace: None,
        skills: None,
    };

    let resolved = resolver
        .resolve(&request, None)
        .await
        .expect("resolve should succeed for subagent lane");

    // `descriptor.agent_id` MUST equal the override so
    // `SharedToolEventSink` scopes lifecycle events correctly.
    assert_eq!(
        resolved.descriptor.agent_id, subagent_agent_id,
        "descriptor.agent_id must honor agent_id_override for subagent lanes"
    );

    // The tool registry MUST expose visible tools for the
    // subagent's agent_id.
    let registry = resolved
        .tool_registry
        .as_ref()
        .expect("tool_registry must be Some for a subagent lane");
    let filter = registry.filter_for(&subagent_agent_id);
    let visible = filter.visible_tools();
    assert!(
        !visible.is_empty(),
        "subagent lane must have visible tools after the build_tool_registry fix"
    );
}

#[tokio::test]
async fn resolve_selects_requested_llm_profile() {
    let temp = tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    let config_path = temp.path().join("config.toml");
    let workspace_str = workspace.to_string_lossy().replace('\\', "\\\\");
    std::fs::write(
        &config_path,
        format!(
            r#"
[llm]
active_profile = "primary"

[llm.profiles.primary]
provider = "ollama"
model = "llama3"
api_base = "http://127.0.0.1:1"

[llm.profiles.alternate]
provider = "ollama"
model = "qwen2.5-coder"
api_base = "http://127.0.0.1:1"
max_tokens = 4096
kvcache_enabled = true
reasoning_effort = "high"

[[agents.list]]
id = "main"
default = true
workspace = "{workspace_str}"
"#
        ),
    )
    .expect("write profile config");
    let config = DaemonConfig::load_from(&config_path).expect("load profile config");
    let resolver = ConfiguredRuntimeResolver::from_config(&config)
        .await
        .expect("construct resolver");
    let request = SessionRuntimeBuildInput {
        session_id: "profile-session".to_string(),
        conversation_id: "profile-session".to_string(),
        sender_id: "test-user".to_string(),
        channel: None,
        channel_instance_id: None,
        channel_identity_prompt: None,
        entry: GatewayEntryContext::default(),
        agent_id_override: None,
        max_turns_override: None,
        subagent_role_id: None,
        llm: Some(LlmRuntimeConfig {
            profile_id: Some("alternate".to_string()),
            provider: None,
            model: None,
            api_base: None,
            api_key_env: None,
            api_key: None,
            reasoning_effort: None,
        }),
        workspace: None,
        skills: None,
    };

    let resolved = resolver
        .resolve(&request, None)
        .await
        .expect("resolve selected profile");
    assert_eq!(resolved.descriptor.model, "qwen2.5-coder");
    assert_eq!(
        resolved
            .descriptor
            .llm
            .as_ref()
            .and_then(|llm| llm.profile_id.as_deref()),
        Some("alternate")
    );
    assert_eq!(resolved.descriptor.token_budget.reserved_for_output, 4096);
    assert!(resolved.descriptor.feature_flags.kvcache_enabled);
    assert_eq!(
        resolved
            .descriptor
            .llm
            .as_ref()
            .and_then(|llm| llm.reasoning_effort),
        Some(ReasoningEffort::High)
    );
}

/// Minimal `DaemonConfig` for resolver tests. Uses the `ollama`
/// provider with `api_base` pointed at an unreachable loopback port
/// so the context-window probe fails instantly (no network wait)
/// and the static fallback kicks in. The agent's `workspace` is
/// set to a tempdir so the test does not pollute `$HOME/.xiaoo`.
async fn minimal_test_config() -> (DaemonConfig, TempDir) {
    let temp = tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    let config_path = temp.path().join("config.toml");
    let workspace_str = workspace.to_string_lossy().replace('\\', "\\\\");
    std::fs::write(
        &config_path,
        &format!(
            "[llm]\n\
                 provider = \"ollama\"\n\
                 model = \"llama3\"\n\
                 api_base = \"http://127.0.0.1:1\"\n\
                 [[agents.list]]\n\
                 id = \"main\"\n\
                 default = true\n\
                 workspace = \"{workspace_str}\"\n"
        ),
    )
    .expect("write minimal config");
    let config = DaemonConfig::load_from(&config_path).expect("load minimal config");
    (config, temp)
}
