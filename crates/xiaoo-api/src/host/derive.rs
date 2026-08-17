//! `SessionOptions` → `(SessionOpenRequest, HostedSessionRuntimeConfig)` 的派生，
//! 与 §3.3.9 四组 helper 实现。
//!
//! 派生纪律：**不新增运行时行为**。每个字段值都来自现有 endside 组装代码
//! （`cli/entry.rs:956-1038`、`runtime_request.rs:247-326`）的提炼。差异点：
//!
//! - `system_prompt`：§3.3.3 未列出 builder 方法。当前默认空字符串；
//!   endside 翻译层可通过 `subagent_roles` 的 prompt 字段注入。
//! - `max_turns`：默认 `None`（用内核默认），与 TUI 一致；CLI 的 `Some(10)`
//!   由 endside 翻译层设置 `token_budget` 时一并注入（暂未提供 builder 方法）。
//! - `client_id` / `client_pid` / `client_hostname`：自动填充本进程信息
//!   （`client_id = uuid_v4`、`client_pid = std::process::id()`、
//!   `client_hostname` 尽力解析）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use agent_types::common::ids::AgentId;
use agent_types::context::TokenBudgetConfig;
use compact::{build_context_manager, CompactOverrides};
use llm_client::{
    create_llm_provider, get_known_model_context_length, resolve_config,
    resolve_model_context_length, resolve_protocol_family, LlmProviderConfig, LlmProviderWrapper,
    ProtocolFamily, ResolveInput,
};
use mcp::McpServerConfig;
use serde_json::Value;
use skill::SkillsConfig;
use tool::{load_tool_sources_with_services, ToolRuntimeServices};
use xiaoo_shared::gateway::session_record::SubagentRoleRecord;
use xiaoo_shared::gateway::{
    GatewayEntryContext, HostedSessionRuntimeConfig, LlmRuntimeConfig, SessionOpenRequest,
    SessionRuntimeDescriptor, SessionServiceError, SubagentRoleConfigEntry,
};

use crate::config::builtin_agent_roles::{PLAN_AGENT_DESCRIPTION, PLAN_AGENT_ID, PLAN_AGENT_PROMPT};
use crate::host::options::{SessionOptions, SkillsSection};

/// 内置 plan 角色的工具开关表（取自 endside `support/config.rs:450-456`）。
const PLAN_AGENT_TOOLS: &[(&str, bool)] = &[
    ("bash", false),
    ("file_edit", false),
    ("file_write", false),
    ("send_file", false),
    ("spawn_subagent", false),
];

/// 派生 `SessionOptions` 为 `(SessionOpenRequest, HostedSessionRuntimeConfig)`。
///
/// 派生规则见 `refactor.md` §3.3.3 派生规则表。错误折叠进
/// [`SessionServiceError`]（§3.3.6）：构建失败 → `RuntimeBuild`；覆盖内置
/// plan 角色 → `InvalidRequest`。
pub(crate) async fn derive_session(
    options: SessionOptions,
) -> Result<(SessionOpenRequest, HostedSessionRuntimeConfig), SessionServiceError> {
    let llm = options.llm;
    let workspace_root = options
        .workspace_root
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // agent_id：默认 "defaultagent"（与 endside CLI 一致）。
    let agent_role = options
        .agent_role
        .clone()
        .unwrap_or_else(|| "defaultagent".to_string());

    // 内置 plan 角色 + 用户额外角色合并；不允许覆盖 plan。
    if options.subagent_roles.contains_key(PLAN_AGENT_ID) {
        return Err(SessionServiceError::InvalidRequest {
            message: format!(
                "agent role `{PLAN_AGENT_ID}` is builtin and cannot be overridden"
            ),
        });
    }
    let mut subagent_roles: BTreeMap<String, SubagentRoleConfigEntry> = BTreeMap::new();
    subagent_roles.insert(
        PLAN_AGENT_ID.to_string(),
        SubagentRoleConfigEntry {
            description: PLAN_AGENT_DESCRIPTION.to_string(),
            prompt: Some(PLAN_AGENT_PROMPT.to_string()),
            max_turns: None,
            tools: PLAN_AGENT_TOOLS
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
        },
    );
    for (role_id, entry) in options.subagent_roles {
        subagent_roles.insert(role_id, entry);
    }

    // API key 解析优先级（§3.3.3 派生规则）：
    //   api_key 直供 > api_key_env 经 get_decrypted_api_key 按需解密 > provider 惯例
    let resolved_api_key = if let Some(key) = llm.api_key.clone() {
        Some(key)
    } else if let Some(env_name) = llm.api_key_env.as_deref() {
        let trimmed = env_name.trim();
        if trimmed.is_empty() {
            None
        } else {
            crate::secrets::get_decrypted_api_key(trimmed).filter(|s| !s.trim().is_empty())
        }
    } else {
        None
    };

    // LLM provider 实例（§3.3.9 build_llm_provider；用于 capabilities + 压缩管道）。
    let llm_provider = build_llm_provider(
        &llm.provider,
        &llm.model,
        resolved_api_key.as_deref(),
        llm.api_base.as_deref(),
        Some(agent_role.clone()),
    )
    .map_err(|e| SessionServiceError::RuntimeBuild {
        message: format!("failed to build LLM provider: {e}"),
    })?;

    // 压缩管道（§3.3.9 build_compression_pipeline；缺省 CompactOverrides::default()）。
    let compression_pipeline = build_compression_pipeline(CompactOverrides::default(), &llm_provider)
        .map_err(|e| SessionServiceError::RuntimeBuild {
            message: format!("failed to build compression pipeline: {e}"),
        })?;

    // 上下文窗口（§3.3.9 resolve_context_window）。
    let total_budget = resolve_context_window(
        &llm.provider,
        &llm.model,
        llm.api_base.as_deref(),
        llm.context_window,
    )
    .await;
    let reserved_for_output = total_budget / 10;
    let reserved_for_system = total_budget / 20;

    // 可见工具（§3.3.9 resolve_visible_tool_names；空开关表 → None 全开）。
    let visible_tool_names = resolve_visible_tool_names(workspace_root.clone(), options.tools);

    // skills config（§3.3.9 resolve_skills_config；显式 skills() 跳过四级优先级）。
    let skills_config = match options.skills {
        Some(dirs) => SkillsConfig {
            skills_dirs: dirs,
            allow_scripts: false,
            ..SkillsConfig::default()
        },
        None => resolve_skills_config(workspace_root.clone(), options.skills_section),
    };

    // MCP servers：显式 > 空 Vec（与 endside CLI 的 `mcp_servers: Vec::new()` 一致）。
    let mcp_servers: Vec<McpServerConfig> = options.mcp_servers.unwrap_or_default();

    // feature_flags / token_budget：§3.3.3 派生规则 "类型 Default / 派生"。
    let feature_flags = options.feature_flags.unwrap_or_default();
    let token_budget = options.token_budget.unwrap_or_else(|| TokenBudgetConfig {
        total_budget,
        reserved_for_output,
        reserved_for_system,
        hard_limit_ratio: 0.9,
    });

    // entry：缺省 cli()（§3.3.3 派生规则）。
    let entry = options.entry.unwrap_or_else(GatewayEntryContext::cli);

    // session_id：缺省生成新 uuid（§3.3.3 派生规则）。
    let session_id = options
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // 租约字段：自动填充本进程信息（§3.3.3 派生规则）。
    let client_id = Some(uuid::Uuid::new_v4().to_string());
    let client_pid = Some(std::process::id());
    let client_hostname = hostname();

    // system_prompt：§3.3.3 未列 builder 方法。默认空字符串；
    // endside 翻译层可通过 subagent_roles 的 prompt 注入。
    let system_prompt = String::new();

    // SessionOpenRequest（对照 endside `runtime_request.rs:329-345`）。
    let open_request = SessionOpenRequest {
        session_id: session_id.clone(),
        conversation_id: session_id.clone(),
        sender_id: agent_role.clone(),
        entry,
        channel: None,
        channel_instance_id: None,
        llm: None,
        workspace: None,
        skills: None,
        client_id,
        client_pid,
        client_hostname,
    };

    // HostedSessionRuntimeConfig（对照 endside `cli/entry.rs:956-1038`）。
    let runtime_config = HostedSessionRuntimeConfig {
        descriptor: SessionRuntimeDescriptor {
            agent_id: AgentId(agent_role),
            model: llm.model.clone(),
            llm: Some(LlmRuntimeConfig {
                provider: Some(llm.provider.clone()),
                model: Some(llm.model.clone()),
                api_base: llm.api_base.clone(),
                api_key_env: llm.api_key_env.clone(),
                api_key: None,
            }),
            system_prompt,
            feature_flags,
            token_budget,
            workspace_root,
            max_turns: None,
            subagent_roles: subagent_roles
                .iter()
                .map(|(role_id, entry)| {
                    (
                        role_id.clone(),
                        SubagentRoleRecord {
                            role_id: role_id.clone(),
                            description: entry.description.clone(),
                            prompt: entry.prompt.clone(),
                            max_turns: entry.max_turns,
                            tools: entry.tools.clone(),
                        },
                    )
                })
                .collect(),
        },
        provider: llm.provider.clone(),
        model: llm.model.clone(),
        api_key: resolved_api_key,
        api_key_env: llm.api_key_env.clone(),
        api_base: llm.api_base.clone(),
        visible_tool_names,
        compression_pipeline: Some(compression_pipeline),
        llm_provider: Some(llm_provider),
        trace: Value::Object(serde_json::Map::new()),
        hooker: Default::default(),
        lsp_registry: options.lsp_registry,
        operation_backend: options.backend,
        skills_config,
        subagent_roles,
        mcp_servers,
        memory_automation: Default::default(),
    };

    Ok((open_request, runtime_config))
}

// ---------------------------------------------------------------------------
// §3.3.9 内部 helper（pub(crate)）
// ---------------------------------------------------------------------------

/// 可见工具枚举：内置工具枚举 ∩ 角色 `tools` 开关；无开关表则全开
/// （返回 `None`）。提取自 `runtime_request.rs:431-459`。
///
/// `tools_override` 中的键必须为内置工具枚举的精确名字
/// （`load_tool_sources_with_services` 返回的 `tool.spec.name()`）；
/// 不在枚举里的配置项被静默忽略。
pub(crate) fn resolve_visible_tool_names(
    workspace_root: PathBuf,
    tools_override: BTreeMap<String, bool>,
) -> Option<Vec<String>> {
    if tools_override.is_empty() {
        return None;
    }

    let all_tool_names: BTreeSet<String> = load_tool_sources_with_services(ToolRuntimeServices {
        workspace_root: Some(workspace_root),
        ..ToolRuntimeServices::default()
    })
    .iter()
    .flat_map(|source| source.discover())
    .map(|tool| tool.spec.name().0.clone())
    .collect();
    let mut visible_tool_names = all_tool_names.clone();

    for (configured_name, enabled) in &tools_override {
        if !all_tool_names.contains(configured_name) {
            continue;
        }
        if *enabled {
            visible_tool_names.insert(configured_name.clone());
        } else {
            visible_tool_names.remove(configured_name);
        }
    }

    Some(visible_tool_names.into_iter().collect())
}

/// skills 目录四级优先级解析。提取自 `support/config.rs:330-370` 与
/// `cli/entry.rs:340-436` 两份合一（两份实现行为一致——见 tests 锁定）。
///
/// 优先级：
/// 1. 项目级（`.xiaoo/skills`，相对进程 CWD）
/// 2. 用户配置目录（来自 `section.dirs`，去重）
/// 3. 用户级（`$HOME/.xiaoo/skills`）
/// 4. 系统级（`/usr/lib/.xiaoo/skills`，内置 skills 如 xiaoo-guardian）
///
/// `allow_scripts` 来自 `section.allow_scripts`，缺省 `false`。
pub(crate) fn resolve_skills_config(
    _workspace_root: PathBuf,
    section: Option<SkillsSection>,
) -> SkillsConfig {
    let mut skills_dirs = Vec::new();

    // Priority 1: Project level (highest)
    skills_dirs.push(PathBuf::from(".xiaoo/skills"));

    // Priority 2: Config file user dirs (medium)
    if let Some(skills) = section.as_ref() {
        for dir in &skills.dirs {
            let path = PathBuf::from(dir);
            let dir_str = path.to_string_lossy();
            if dir_str != ".xiaoo/skills"
                && !dir_str.ends_with("/.xiaoo/skills")
                && !dir_str.ends_with("\\.xiaoo\\skills")
                && dir_str != "/usr/lib/.xiaoo/skills"
            {
                skills_dirs.push(path);
            }
        }
    }

    // Priority 3: User level
    if let Some(home) = dirs::home_dir() {
        skills_dirs.push(home.join(".xiaoo").join("skills"));
    }

    // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
    skills_dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

    SkillsConfig {
        skills_dirs,
        allow_scripts: section
            .as_ref()
            .map(|s| s.allow_scripts)
            .unwrap_or(false),
        ..SkillsConfig::default()
    }
}

/// 上下文窗口解析链。提取自 `support/config.rs:560-571` 与
/// `cli/mod.rs:182-195` 两份合一（差异见下）。
///
/// 优先级（§3.3.3 派生规则）：
/// 1. 显式 `explicit`（最高优先，跳过探测）
/// 2. `resolve_model_context_length` 动态探测（内部：catalog > known > ollama）
/// 3. `get_known_model_context_length` 已知模型表
/// 4. `resolve_protocol_family` 协议族缺省
///
/// **差异点**（与原 `cli/mod.rs:182-195` 的 `resolve_effective_context_window` 对比）：
/// 原函数在所有探测失败后最终回退到 `llm_provider.capabilities().max_context_window`；
/// 本 helper 不接受 `LlmProvider` 实例（签名只取 provider/model/api_base/explicit），
/// 故协议族缺省即为最后一站。这是有意的取舍——派生阶段不一定已有 provider
/// 实例；调用方需要 provider capability 兜底时，可在派生后用
/// `runtime_config.llm_provider.as_ref().map(|p| p.capabilities().max_context_window)`
/// 单独处理（与 `resolve_effective_context_window` 的等价路径）。
pub(crate) async fn resolve_context_window(
    provider: &str,
    model: &str,
    api_base: Option<&str>,
    explicit: Option<u32>,
) -> usize {
    if let Some(explicit) = explicit {
        return explicit as usize;
    }

    // 动态探测（catalog > known > ollama fallback）
    let resolved = match resolve_config(ResolveInput {
        provider: Some(provider.to_string()),
        protocol: None,
        api_key: None,
        api_key_env: None,
        base_url: api_base.map(|s| s.to_string()),
    }) {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::warn!(
                provider = %provider,
                model = %model,
                error = %error,
                "failed to resolve provider config for context window lookup; falling back to static lookup",
            );
            // 直接走静态 known table + 协议族缺省
            return static_context_window(provider, model);
        }
    };

    match resolve_model_context_length(&resolved, model).await {
        Ok(Some(context_window)) => match usize::try_from(context_window) {
            Ok(value) if value > 0 => return value,
            Ok(_) => {
                tracing::warn!(
                    provider = %provider,
                    model = %model,
                    context_window,
                    "dynamic context window does not fit usize; falling back",
                );
            }
            Err(_) => {
                tracing::warn!(
                    provider = %provider,
                    model = %model,
                    context_window,
                    "dynamic context window does not fit usize; falling back",
                );
            }
        },
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                provider = %provider,
                model = %model,
                error = %error,
                "failed to dynamically resolve context window; falling back to static lookup",
            );
        }
    }

    static_context_window(provider, model)
}

/// 静态回退：known 模型表 > 协议族缺省 > `1`（保底非零）。
/// 提取自 `support/config.rs:560-571`。
fn static_context_window(provider: &str, model: &str) -> usize {
    if let Some(known) = get_known_model_context_length(model) {
        if let Ok(value) = usize::try_from(known) {
            if value > 0 {
                return value;
            }
        }
    }

    match resolve_protocol_family(provider) {
        Some(ProtocolFamily::OpenAiCompatible | ProtocolFamily::Ollama | ProtocolFamily::Zhipu) => {
            128_000
        }
        Some(ProtocolFamily::Anthropic) => 200_000,
        Some(ProtocolFamily::Gemini) => 1_000_000,
        None => 1,
    }
}

/// 构建 LLM provider 实例。提取自 `cli/mod.rs:140-155`。
///
/// `api_key` 为已解析的实际 key（直供或经 secrets 解密后的明文）；
/// `None` 表示交由 `create_llm_provider` 内部按 provider 惯例环境变量解析。
pub(crate) fn build_llm_provider(
    provider: &str,
    model: &str,
    api_key: Option<&str>,
    api_base: Option<&str>,
    agent_id: Option<String>,
) -> Result<Arc<LlmProviderWrapper>, llm_client::LlmError> {
    let mut config = LlmProviderConfig::new(provider, model);
    if let Some(key) = api_key {
        config = config.with_api_key(key);
    }
    if let Some(base) = api_base {
        config = config.with_api_base(base);
    }
    Ok(Arc::new(create_llm_provider(&config, agent_id, None)?))
}

/// 构建压缩管道。提取自 `cli/mod.rs:157-180`。
///
/// 走 `compact::build_context_manager` 与 daemon 共用同一份默认值与 estimator 调参。
/// `overrides` 字段全 `None` 时使用 `build_context_manager` 内置默认值。
pub(crate) fn build_compression_pipeline(
    overrides: CompactOverrides,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> Result<Arc<dyn agent_contracts::CompressionPipeline>, compact::CompactError> {
    let pipeline = build_context_manager(Some(&overrides), Arc::clone(llm_provider))?;
    Ok(pipeline)
}

/// 尽力解析本机 hostname；纯展示用，`None` 表示解析失败。
/// 提取自 endside `runtime_request.rs:381-393`。
fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .and_then(|v| {
                let trimmed = v.trim().to_string();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .or_else(|| {
                std::env::var("HOSTNAME").ok().and_then(|v| {
                    let trimmed = v.trim().to_string();
                    (!trimmed.is_empty()).then_some(trimmed)
                })
            })
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").ok().and_then(|v| {
            let trimmed = v.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        })
    }
}
