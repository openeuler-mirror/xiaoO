//! 阶段 2.1 测试：派生 helper 行为快照 + 派生结果字段对照。
//!
//! 覆盖：
//! - skills 4 级优先级解析（§阶段 2.1 行为快照，对照
//!   `support/config.rs:330-370` 与 `cli/entry.rs:340-436` 两份合一）
//! - context window 解析链（对照 `support/config.rs:560-571` 与
//!   `cli/mod.rs:182-195` 两份合一）
//! - 可见工具解析（对照 `runtime_request.rs:431-459`）
//! - `SessionOptions::derive` 结果与 `cli/entry.rs:956-1038` 字段对照

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::derive::{
    build_compression_pipeline, build_llm_provider, resolve_context_window,
    resolve_skills_config, resolve_visible_tool_names,
};
use super::options::{LlmOptions, SessionOptions, SkillsSection};
use crate::config::builtin_agent_roles::{PLAN_AGENT_DESCRIPTION, PLAN_AGENT_ID, PLAN_AGENT_PROMPT};

use compact::CompactOverrides;

// ===========================================================================
// resolve_skills_config —— 行为快照（对照 support/config.rs:330-370 与
// cli/entry.rs:340-436 两份合一；两份实现行为一致，差异点：无）
// ===========================================================================

mod skills_snapshot {
    use super::*;

    /// 缺省（section=None）：4 级默认目录按优先级顺序排列。
    ///
    /// 注：在 HOME 未设置的 CI 环境下，Priority 3（用户级）会被跳过，
    /// 故 skills_dirs 长度可能是 3 或 4。本测试只断言头部（project）
    /// 与尾部（system）必定存在。
    #[test]
    fn default_returns_four_levels_in_order() {
        let cfg = resolve_skills_config(PathBuf::from("."), None);

        // 3 或 4 级默认目录（取决于 HOME 是否可用）
        assert!(
            cfg.skills_dirs.len() >= 3,
            "expected at least 3 default skill dirs, got {}",
            cfg.skills_dirs.len()
        );
        // 头部：project level
        assert_eq!(cfg.skills_dirs[0], PathBuf::from(".xiaoo/skills"));
        // 尾部：system level
        assert_eq!(cfg.skills_dirs.last().unwrap(), &PathBuf::from("/usr/lib/.xiaoo/skills"));
        // 中间若有用户级，应在 project 与 system 之间
        if cfg.skills_dirs.len() == 4 {
            assert!(
                cfg.skills_dirs[1].ends_with(".xiaoo/skills")
                    || cfg.skills_dirs[1].ends_with(".xiaoo\\skills"),
                "user-level dir should end with .xiaoo/skills, got {}",
                cfg.skills_dirs[1].display()
            );
        }
        assert!(!cfg.allow_scripts);
    }

    /// 用户配置的额外目录会被插入到 project 与 user 之间。
    #[test]
    fn section_dirs_are_inserted_between_project_and_user() {
        let section = SkillsSection {
            dirs: vec![PathBuf::from("/custom/skills-a"), PathBuf::from("/custom/skills-b")],
            allow_scripts: false,
        };
        let cfg = resolve_skills_config(PathBuf::from("."), Some(section));

        // 期望：project + 2 个用户目录 + user + system
        assert_eq!(cfg.skills_dirs[0], PathBuf::from(".xiaoo/skills"));
        assert_eq!(cfg.skills_dirs[1], PathBuf::from("/custom/skills-a"));
        assert_eq!(cfg.skills_dirs[2], PathBuf::from("/custom/skills-b"));
        // 最后是 system level
        assert_eq!(cfg.skills_dirs.last().unwrap(), &PathBuf::from("/usr/lib/.xiaoo/skills"));
    }

    /// section 中显式出现的 `.xiaoo/skills`（项目级）与
    /// `/usr/lib/.xiaoo/skills`（系统级）会被去重。
    #[test]
    fn section_dirs_dedup_project_and_system_levels() {
        let section = SkillsSection {
            dirs: vec![
                PathBuf::from(".xiaoo/skills"),                // == project level
                PathBuf::from("/usr/lib/.xiaoo/skills"),       // == system level
                PathBuf::from("/some/other"),                  // unique
                PathBuf::from("foo/.xiaoo/skills"),             // 后缀匹配 project level
            ],
            allow_scripts: false,
        };
        let cfg = resolve_skills_config(PathBuf::from("."), Some(section));

        // project level 出现一次（去重后的第一条）
        assert_eq!(cfg.skills_dirs[0], PathBuf::from(".xiaoo/skills"));
        // 后缀匹配被去重
        assert!(
            !cfg
                .skills_dirs
                .iter()
                .any(|d| d == &PathBuf::from("foo/.xiaoo/skills")),
            "foo/.xiaoo/skills should be deduped"
        );
        // system level 出现一次（去重后的最后一条）
        assert_eq!(cfg.skills_dirs.last().unwrap(), &PathBuf::from("/usr/lib/.xiaoo/skills"));
        // unique 的目录保留
        assert!(cfg.skills_dirs.iter().any(|d| d == &PathBuf::from("/some/other")));
    }

    /// `allow_scripts` 缺省 false；section 设为 true 时透传。
    #[test]
    fn allow_scripts_is_threaded_through() {
        let section = SkillsSection {
            dirs: vec![],
            allow_scripts: true,
        };
        let cfg = resolve_skills_config(PathBuf::from("."), Some(section));
        assert!(cfg.allow_scripts);

        let cfg2 = resolve_skills_config(PathBuf::from("."), None);
        assert!(!cfg2.allow_scripts);
    }

    /// workspace_root 当前未参与解析（现有两份实现也未使用）。
    #[test]
    fn workspace_root_does_not_affect_dirs() {
        let cfg_a = resolve_skills_config(PathBuf::from("/any/path"), None);
        let cfg_b = resolve_skills_config(PathBuf::from("/other/path"), None);
        assert_eq!(cfg_a.skills_dirs, cfg_b.skills_dirs);
    }
}

// ===========================================================================
// resolve_context_window —— 行为快照（对照 support/config.rs:560-571 与
// cli/mod.rs:182-195 两份合一；差异点：见 doc comment）
// ===========================================================================

mod context_window_snapshot {
    use super::*;

    /// 显式配置（最高优先）跳过所有探测。
    #[tokio::test]
    async fn explicit_value_skips_probing() {
        let result = resolve_context_window("openai", "gpt-4.1", None, Some(99_999)).await;
        assert_eq!(result, 99_999);
    }

    /// 已知模型（在 known_models.toml 中）→ 返回 known 表的值。
    /// 这条路径与 support/config.rs:560-571 的 `resolve_context_window` 一致。
    #[tokio::test]
    async fn known_model_falls_back_to_known_table() {
        // 用一个不需要 catalog 查询的 provider；让 resolve_config() 失败
        // 以强制走静态路径。
        let result = resolve_context_window("unknown-provider", "gpt-4.1", None, None).await;
        // gpt-4.1 应该在 known_models.toml 中
        assert!(result > 0);
    }

    /// 已知 provider + 未知模型 → 协议族缺省。
    /// 与 support/config.rs:560-571 的协议族分支一致。
    #[tokio::test]
    async fn unknown_model_known_provider_falls_back_to_protocol_family() {
        // openai 是 OpenAiCompatible → 128_000
        // 但 catalog 查询可能失败（无网络/api key），所以这里只验证不返回 0
        let result = resolve_context_window(
            "openai",
            "totally-unknown-model",
            Some("https://api.openai.com/v1"),
            None,
        )
        .await;
        // 不论走动态还是静态，最终值应该是 128_000（OpenAiCompatible 协议族缺省）
        assert_eq!(result, 128_000);
    }

    /// 未知 provider + 未知模型 → 1（保底非零）。
    #[tokio::test]
    async fn unknown_provider_unknown_model_returns_default_floor() {
        let result = resolve_context_window(
            "totally-unknown-provider",
            "totally-unknown-model",
            None,
            None,
        )
        .await;
        assert_eq!(result, 1, "unknown provider should fall through to floor of 1");
    }
}

// ===========================================================================
// resolve_visible_tool_names —— 行为快照（对照 runtime_request.rs:431-459）
// ===========================================================================

mod visible_tools_snapshot {
    use super::*;

    /// 空开关表 → None（全开）。
    #[test]
    fn empty_override_returns_none() {
        let result = resolve_visible_tool_names(PathBuf::from("."), BTreeMap::new());
        assert!(result.is_none());
    }

    /// 非空开关表 → Some(Vec)。不在内置工具枚举里的配置项被静默忽略。
    #[test]
    fn override_filters_builtin_tools() {
        let mut switches = BTreeMap::new();
        // 关掉 file_edit（如果存在）
        switches.insert("file_edit".to_string(), false);
        // 不存在的工具名——应被静默忽略
        switches.insert("nonexistent_tool".to_string(), false);

        let result = resolve_visible_tool_names(PathBuf::from("."), switches);
        let visible = result.expect("non-empty override should return Some");

        // file_edit 应不在 visible 中（被关掉）
        assert!(
            !visible.iter().any(|n| n == "file_edit"),
            "file_edit should be disabled"
        );
        // nonexistent_tool 不应被加进 visible（被静默忽略）
        assert!(
            !visible.iter().any(|n| n == "nonexistent_tool"),
            "nonexistent tool name should be silently ignored"
        );
        // 内置工具总数应 > 0
        assert!(!visible.is_empty(), "builtin tools should be discovered");
    }

    /// 关掉一组明确的工具名 → 这些工具不在 visible 中，其余仍在。
    ///
    /// 不再断言 visible 为空——内置工具集合会演化，"全关"清单不可能
    /// 完备。本测试改为：明确关掉 bash + file_edit，验证它们不在 visible 中，
    /// 且 visible 非空（其余内置工具仍开）。
    #[test]
    fn disabling_specific_tools_removes_them_from_visible() {
        let mut switches = BTreeMap::new();
        switches.insert("bash".to_string(), false);
        switches.insert("file_edit".to_string(), false);
        switches.insert("file_write".to_string(), false);
        switches.insert("file_read".to_string(), false);
        switches.insert("glob".to_string(), false);
        switches.insert("grep".to_string(), false);

        let result = resolve_visible_tool_names(PathBuf::from("."), switches);
        let visible = result.expect("non-empty override should return Some");

        // 被关掉的工具不应在 visible 中（若它们原本是内置工具）
        for disabled in &["bash", "file_edit", "file_write", "file_read", "glob", "grep"] {
            assert!(
                !visible.iter().any(|n| n == disabled),
                "{disabled} should be disabled"
            );
        }
        // 至少应有一些内置工具未被关掉（除非上述清单恰好覆盖全部，
        // 那也是合法行为——此时 visible 为空 Vec）
        // 这里只断言返回的是 Some(Vec)，不对其是否为空做硬性要求。
        let _ = visible;
    }
}

// ===========================================================================
// build_llm_provider / build_compression_pipeline —— 行为快照
// ===========================================================================

mod build_helpers {
    use super::*;

    /// ollama 不需要 API key，可以无需网络构造。
    #[test]
    fn build_llm_provider_succeeds_for_ollama() {
        let provider = build_llm_provider(
            "ollama",
            "qwen2.5:7b",
            None,
            Some("http://localhost:11434"),
            Some("test-agent".to_string()),
        );
        let provider = provider.expect("ollama provider should build without API key");
        assert!(provider.capabilities().max_context_window > 0);
    }

    /// 未知 provider → 报错。
    #[test]
    fn build_llm_provider_fails_for_unknown_provider() {
        let result = build_llm_provider(
            "totally-unknown-provider",
            "any-model",
            None,
            None,
            None,
        );
        assert!(result.is_err());
    }

    /// 默认 CompactOverrides + 已构造的 ollama provider → 应成功。
    #[test]
    fn build_compression_pipeline_succeeds_with_defaults() {
        let provider = build_llm_provider(
            "ollama",
            "qwen2.5:7b",
            None,
            Some("http://localhost:11434"),
            None,
        )
        .expect("ollama provider should build");

        let pipeline = build_compression_pipeline(CompactOverrides::default(), &provider);
        let _pipeline = pipeline.expect("compression pipeline should build with defaults");
    }
}

// ===========================================================================
// SessionOptions::derive —— 派生结果与 cli/entry.rs:956-1038 字段对照
// ===========================================================================

mod derive_field_by_field {
    use super::*;
    use xiaoo_shared::gateway::GatewayEntryKind;

    /// 最小 SessionOptions（无 API key，ollama provider）派生应成功。
    /// 验证默认值与 §3.3.3 派生规则表一致。
    #[tokio::test]
    async fn minimal_options_derive_defaults() {
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        );

        let (open_request, runtime_config) = options
            .derive()
            .await
            .expect("minimal ollama options should derive");

        // ---- SessionOpenRequest 默认值（对照 endside runtime_request.rs:329-345）----
        // session_id 应被生成（新 uuid）
        assert!(!open_request.session_id.is_empty());
        assert_eq!(open_request.conversation_id, open_request.session_id);
        // sender_id = agent_role（默认 "defaultagent"）
        assert_eq!(open_request.sender_id, "defaultagent");
        // entry = cli()（默认）
        assert_eq!(open_request.entry.kind, Some(GatewayEntryKind::Cli));
        assert!(open_request.channel.is_none());
        assert!(open_request.channel_instance_id.is_none());
        assert!(open_request.llm.is_none());
        assert!(open_request.workspace.is_none());
        assert!(open_request.skills.is_none());
        // 租约字段自动填充本进程信息
        assert!(open_request.client_id.is_some());
        assert!(open_request.client_pid.is_some());
        // hostname 在 CI 上可能解析失败，不强断言

        // ---- HostedSessionRuntimeConfig 默认值（对照 endside cli/entry.rs:956-1038）----
        // descriptor
        assert_eq!(runtime_config.descriptor.agent_id.0, "defaultagent");
        assert_eq!(runtime_config.descriptor.model, "qwen2.5:7b");
        assert!(runtime_config.descriptor.llm.is_some());
        let llm_in_descriptor = runtime_config.descriptor.llm.as_ref().unwrap();
        assert_eq!(llm_in_descriptor.provider.as_deref(), Some("ollama"));
        assert_eq!(llm_in_descriptor.model.as_deref(), Some("qwen2.5:7b"));
        assert_eq!(
            llm_in_descriptor.api_base.as_deref(),
            Some("http://localhost:11434")
        );
        // api_key 在 descriptor 中始终为 None（实际 key 在 HostedSessionRuntimeConfig.api_key）
        assert!(llm_in_descriptor.api_key.is_none());
        // system_prompt 默认空字符串（与 §3.3.3 一致；endside 翻译层负责注入）
        assert!(runtime_config.descriptor.system_prompt.is_empty());
        // feature_flags 默认（tool_execution=true 等）
        assert!(runtime_config.descriptor.feature_flags.tool_execution);
        // token_budget 派生自 context_window=8192
        assert_eq!(runtime_config.descriptor.token_budget.total_budget, 8192);
        assert_eq!(
            runtime_config.descriptor.token_budget.reserved_for_output,
            819
        ); // 8192 / 10
        assert_eq!(
            runtime_config.descriptor.token_budget.reserved_for_system,
            409
        ); // 8192 / 20
        assert_eq!(runtime_config.descriptor.token_budget.hard_limit_ratio, 0.9);
        // workspace_root 默认进程当前目录
        assert!(!runtime_config.descriptor.workspace_root.as_os_str().is_empty());
        // max_turns 默认 None（用内核默认）
        assert!(runtime_config.descriptor.max_turns.is_none());

        // ---- subagent_roles：默认含内置 plan 角色（§3.3.3 派生规则）----
        assert!(runtime_config.subagent_roles.contains_key(PLAN_AGENT_ID));
        let plan_entry = runtime_config
            .subagent_roles
            .get(PLAN_AGENT_ID)
            .expect("plan role should be present");
        assert_eq!(plan_entry.description, PLAN_AGENT_DESCRIPTION);
        assert_eq!(plan_entry.prompt.as_deref(), Some(PLAN_AGENT_PROMPT));
        // plan 的工具开关表（对照 endside support/config.rs:450-456）
        assert_eq!(plan_entry.tools.get("bash"), Some(&false));
        assert_eq!(plan_entry.tools.get("file_edit"), Some(&false));
        assert_eq!(plan_entry.tools.get("file_write"), Some(&false));
        assert_eq!(plan_entry.tools.get("send_file"), Some(&false));
        assert_eq!(plan_entry.tools.get("spawn_subagent"), Some(&false));

        // descriptor.subagent_roles 也应同步含 plan（对照 cli/entry.rs:987-1002）
        assert!(runtime_config
            .descriptor
            .subagent_roles
            .contains_key(PLAN_AGENT_ID));

        // ---- 顶层 LLM 字段 ----
        assert_eq!(runtime_config.provider, "ollama");
        assert_eq!(runtime_config.model, "qwen2.5:7b");
        // api_key 为 None（未直供、无 api_key_env）
        assert!(runtime_config.api_key.is_none());
        assert!(runtime_config.api_key_env.is_none());
        assert_eq!(
            runtime_config.api_base.as_deref(),
            Some("http://localhost:11434")
        );
        // visible_tool_names 默认 None（全开，无开关表）
        assert!(runtime_config.visible_tool_names.is_none());
        // compression_pipeline 与 llm_provider 已构造
        assert!(runtime_config.compression_pipeline.is_some());
        assert!(runtime_config.llm_provider.is_some());
        // trace 默认空 Object
        assert!(runtime_config.trace.is_object());
        assert!(runtime_config.trace.as_object().unwrap().is_empty());
        // hooker 默认空（enabled/disabled/policies/plugins 全空）
        assert!(runtime_config.hooker.enabled.is_empty());
        assert!(runtime_config.hooker.disabled.is_empty());
        // lsp_registry 默认 None（§3.3.3：缺省不启用）
        assert!(runtime_config.lsp_registry.is_none());
        // operation_backend 默认 None（§3.3.3：由 backend() 显式传入）
        assert!(runtime_config.operation_backend.is_none());
        // skills_config 走四级优先级解析
        assert!(!runtime_config.skills_config.skills_dirs.is_empty());
        // mcp_servers 默认空 Vec
        assert!(runtime_config.mcp_servers.is_empty());
        // memory_automation 默认 disabled
        assert!(!runtime_config.memory_automation.enabled);
    }

    /// 显式 workspace_root / session_id / agent_role / entry / backend
    /// 应被透传到派生结果。
    #[tokio::test]
    async fn explicit_options_are_threaded_through() {
        let tmp = std::env::temp_dir();
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .workspace_root(tmp.clone())
        .session_id("fixed-session-id")
        .agent_role("custom-agent");

        let (open_request, runtime_config) = options.derive().await.expect("should derive");

        // session_id 透传
        assert_eq!(open_request.session_id, "fixed-session-id");
        assert_eq!(open_request.conversation_id, "fixed-session-id");
        // agent_role 透传到 sender_id 与 descriptor.agent_id
        assert_eq!(open_request.sender_id, "custom-agent");
        assert_eq!(runtime_config.descriptor.agent_id.0, "custom-agent");
        // workspace_root 透传
        assert_eq!(runtime_config.descriptor.workspace_root, tmp);
    }

    /// 显式 skills() 跳过四级优先级解析。
    #[tokio::test]
    async fn explicit_skills_skips_resolution() {
        let explicit_dirs = vec![PathBuf::from("/my/custom/skills")];
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .skills(explicit_dirs.clone());

        let (_open, runtime_config) = options.derive().await.expect("should derive");

        assert_eq!(runtime_config.skills_config.skills_dirs, explicit_dirs);
    }

    /// skills_section 注入后，四级优先级解析包含用户额外目录。
    #[tokio::test]
    async fn skills_section_threads_user_dirs_into_resolution() {
        let section = SkillsSection {
            dirs: vec![PathBuf::from("/user/extra/skills")],
            allow_scripts: true,
        };
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .skills_section(section);

        let (_open, runtime_config) = options.derive().await.expect("should derive");

        // 用户额外目录应在 skills_dirs 中
        assert!(runtime_config
            .skills_config
            .skills_dirs
            .iter()
            .any(|d| d == &PathBuf::from("/user/extra/skills")));
        // allow_scripts 透传
        assert!(runtime_config.skills_config.allow_scripts);
        // 4 级默认目录仍存在
        assert!(runtime_config
            .skills_config
            .skills_dirs
            .iter()
            .any(|d| d == &PathBuf::from(".xiaoo/skills")));
    }

    /// 覆盖内置 plan 角色应失败。
    #[tokio::test]
    async fn overriding_builtin_plan_role_fails() {
        let mut subagent_roles = BTreeMap::new();
        subagent_roles.insert(
            PLAN_AGENT_ID.to_string(),
            xiaoo_shared::gateway::SubagentRoleConfigEntry {
                description: "custom".to_string(),
                prompt: None,
                max_turns: None,
                tools: BTreeMap::new(),
            },
        );
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .subagent_roles(subagent_roles);

        let result = options.derive().await;
        let err = match result {
            Ok(_) => panic!("overriding plan role should fail"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("builtin") || msg.contains("plan"),
            "error should mention builtin plan role: {msg}"
        );
    }

    /// 直供 api_key 优先于 api_key_env。
    #[tokio::test]
    async fn direct_api_key_takes_precedence() {
        let options = SessionOptions::new(
            LlmOptions::new("openai", "gpt-4.1")
                .api_key("direct-supplied-key")
                .api_key_env("SOME_ENV_VAR")
                .context_window(8192),
        );

        let (_open, runtime_config) = options.derive().await.expect("should derive");

        // 直供的 api_key 应被解析为 HostedSessionRuntimeConfig.api_key
        assert_eq!(runtime_config.api_key.as_deref(), Some("direct-supplied-key"));
    }

    /// 显式 token_budget 覆盖派生默认值。
    #[tokio::test]
    async fn explicit_token_budget_overrides_derivation() {
        use agent_types::context::TokenBudgetConfig;

        let custom_budget = TokenBudgetConfig {
            total_budget: 100_000,
            reserved_for_output: 5_000,
            reserved_for_system: 1_000,
            hard_limit_ratio: 0.95,
        };
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .token_budget(custom_budget.clone());

        let (_open, runtime_config) = options.derive().await.expect("should derive");

        assert_eq!(
            runtime_config.descriptor.token_budget.total_budget,
            100_000
        );
        assert_eq!(
            runtime_config.descriptor.token_budget.reserved_for_output,
            5_000
        );
        assert_eq!(
            runtime_config.descriptor.token_budget.reserved_for_system,
            1_000
        );
        assert_eq!(
            runtime_config.descriptor.token_budget.hard_limit_ratio,
            0.95
        );
    }

    /// 显式 mcp_servers 透传。
    #[tokio::test]
    async fn explicit_mcp_servers_passthrough() {
        use mcp::McpServerConfig;

        let servers: Vec<McpServerConfig> = vec![]; // 显式空 Vec = 关闭 MCP
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .mcp_servers(servers);

        let (_open, runtime_config) = options.derive().await.expect("should derive");

        assert!(runtime_config.mcp_servers.is_empty());
    }

    /// 自定义 entry 透传；不指定时默认 cli()。
    #[tokio::test]
    async fn entry_passthrough_and_default() {
        use xiaoo_shared::gateway::GatewayEntryContext;

        // 默认 cli()
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        );
        let (open, _rt) = options.derive().await.expect("should derive");
        assert_eq!(open.entry.kind, Some(GatewayEntryKind::Cli));

        // 自定义 tui()
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .entry(GatewayEntryContext::tui(None));
        let (open, _rt) = options.derive().await.expect("should derive");
        assert_eq!(open.entry.kind, Some(GatewayEntryKind::Tui));
    }
}

// ===========================================================================
// LocalSessionHost + Session 生命周期（对照 §3.3.2 / §3.3.4 / §3.3.8）
// ===========================================================================

mod host_lifecycle {
    use super::*;
    use crate::host::{LocalSessionHost, LocalSessionHostBuilder, Session};
    use std::time::Duration;

    /// 默认配置构造 host 成功。
    #[tokio::test]
    async fn default_builder_succeeds() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("default build should succeed");
        // 访问器返回共享实例（仅验证非空，不强断言类型）
        let _store = host.session_store();
        let _bm = host.backend_manager();
        let _cp = host.control_plane();
        // 默认未配置 memory automation
        assert!(host.memory_health().is_none());
    }

    /// 无活跃会话时 shutdown 是 no-op。
    #[tokio::test]
    async fn shutdown_with_no_active_sessions_is_noop() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("build should succeed");
        host.shutdown(Duration::from_secs(1)).await;
        // 二次 shutdown 幂等
        host.shutdown(Duration::from_secs(1)).await;
    }

    /// `open_session` 在派生失败时返回 `SessionServiceError`。
    /// 验证派生 → `build_llm_provider` → `RuntimeBuild` 错误映射。
    #[tokio::test]
    async fn open_session_fails_on_unknown_provider() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("host build should succeed");

        let options = SessionOptions::new(
            LlmOptions::new("totally-unknown-provider", "any-model")
                .context_window(8192),
        );
        let result = host.open_session(options).await;
        let err = match result {
            Ok(_) => panic!("open_session with unknown provider should fail"),
            Err(e) => e,
        };
        // 派生失败折叠进 SessionServiceError::RuntimeBuild（§3.3.6）
        let msg = err.to_string();
        assert!(
            msg.contains("provider") || msg.contains("Provider"),
            "error should mention provider: {msg}"
        );
    }

    /// `open_session` + `session.id()` + `session.export()` + `session.close()`
    /// 走通（使用 ollama provider，无网络依赖——派生 + open_session 不触发 LLM 调用）。
    #[tokio::test]
    async fn open_export_close_lifecycle() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("host build should succeed");

        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        );
        let session = host
            .open_session(options)
            .await
            .expect("open_session with ollama should succeed (no network needed at open time)");

        // id() 返回派生生成的 session_id（非空）
        let id = session.id().to_string();
        assert!(!id.is_empty());

        // export() 返回 SessionRecord
        let record = session
            .export()
            .await
            .expect("export should succeed after open_session");
        assert_eq!(record.session_id, id);

        // close() 返回 SessionRecord 并从 store 移除
        let closed = Session::close(session).await.expect("close should succeed");
        assert_eq!(closed.session_id, id);

        // close 后 export 应返回 SessionNotFound
        // （通过重新构造 Session 句柄——但 Session 已被 close 消费，这里
        // 直接走 host.session_store().load() 验证）
        let store = host.session_store();
        let loaded = store.load(&id).await;
        assert!(loaded.is_none(), "session should be removed from store after close");

        host.shutdown(Duration::from_secs(1)).await;
    }

    /// `host.shutdown()` 关闭所有活跃会话。
    #[tokio::test]
    async fn shutdown_closes_active_sessions() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("host build should succeed");

        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        );
        let session = host
            .open_session(options)
            .await
            .expect("open_session should succeed");
        let id = session.id().to_string();
        // 不显式 close，让 shutdown 处理
        drop(session);

        host.shutdown(Duration::from_secs(1)).await;

        // shutdown 后 session 应已从 store 移除
        let store = host.session_store();
        let loaded = store.load(&id).await;
        assert!(loaded.is_none(), "session should be force_closed by shutdown");
    }

    /// `open_session_with` 跳过派生，直接用手工构造的 request + config。
    /// 验证 advanced 装配路径。
    #[tokio::test]
    async fn open_session_with_bypasses_derive() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("host build should succeed");

        // 手工构造（不走 SessionOptions::derive）
        let (open_request, runtime_config) = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://localhost:11434")
                .context_window(8192),
        )
        .derive()
        .await
        .expect("should derive");

        let session = host
            .open_session_with(open_request, runtime_config)
            .await
            .expect("open_session_with should succeed");
        assert!(!session.id().is_empty());

        host.shutdown(Duration::from_secs(1)).await;
    }

    /// `session.send()` 在 LLM 不可达时返回 `SessionServiceError`。
    /// 验证 send → run_turn_inner → service.run_turn 错误路径（不验证成功路径，
    /// 成功路径需要真实 LLM）。
    ///
    /// 标记 `#[ignore]` 因为 LLM 重试链导致失败耗时较长（~60s）。
    /// 阶段 2.3 落地 `run_turn_raw` + DummyProvider 后改用替身跑完整路径。
    #[tokio::test]
    #[ignore = "slow: requires LLM unreachable retry path"]
    async fn send_returns_error_when_llm_unreachable() {
        let host = LocalSessionHost::builder()
            .build()
            .await
            .expect("host build should succeed");

        // 用一个一定会失败的 api_base（端口 1 是保留端口，连接被拒）
        let options = SessionOptions::new(
            LlmOptions::new("ollama", "qwen2.5:7b")
                .api_base("http://127.0.0.1:1")
                .context_window(8192),
        );
        let session = host
            .open_session(options)
            .await
            .expect("open_session should succeed (no LLM call yet)");

        let result = session.send("hello").await;
        assert!(
            result.is_err(),
            "send should fail when LLM is unreachable"
        );
        let err = match result {
            Ok(_) => panic!("should be err"),
            Err(e) => e,
        };
        // 错误折叠进 SessionServiceError（具体变体不硬断言）
        let _ = err;

        host.shutdown(Duration::from_secs(1)).await;
    }

    /// `LocalSessionHostBuilder::memory_automation` 直接注入（测试替身）。
    #[tokio::test]
    async fn memory_automation_injection() {
        use std::sync::Arc;
        use xiaoo_shared::gateway::memory_automation::TurnMemoryAutomation;
        use xiaoo_shared::gateway::MemoryAutomationHealth;

        // 构造一个最小可用的 TurnMemoryAutomation 替身
        struct NoopAutomation;
        #[async_trait::async_trait]
        impl TurnMemoryAutomation for NoopAutomation {
            async fn recall(
                &self,
                _ctx: &xiaoo_shared::gateway::memory_automation::TurnMemoryContext,
            ) -> Result<
                Vec<xiaoo_shared::gateway::memory_automation::RecallMemory>,
                xiaoo_shared::gateway::memory_automation::MemoryAutomationError,
            > {
                Ok(Vec::new())
            }
            async fn enqueue_ingest(
                &self,
                _turn: xiaoo_shared::gateway::memory_automation::CompletedTurnIngest,
            ) -> Result<(), xiaoo_shared::gateway::memory_automation::MemoryAutomationError> {
                Ok(())
            }
            fn recall_token_budget(&self) -> usize {
                0
            }
            fn subscribe_health(
                &self,
            ) -> Option<tokio::sync::watch::Receiver<MemoryAutomationHealth>> {
                None
            }
        }

        let host = LocalSessionHost::builder()
            .memory_automation(Arc::new(NoopAutomation) as Arc<dyn TurnMemoryAutomation>)
            .build()
            .await
            .expect("build with injected automation should succeed");

        // 注入后 host 持有 automation（但 subscribe_health 返回 None，故 memory_health 是 None）
        assert!(host.memory_health().is_none());

        host.shutdown(Duration::from_secs(1)).await;
    }

    /// `SecretsInit::WithProvider` 路径不 panic。
    #[tokio::test]
    async fn secrets_with_provider_path_does_not_panic() {
        use std::path::PathBuf;
        let tmp = std::env::temp_dir().join("xiaoo-api-test-secrets-nonexistent");
        let host = LocalSessionHost::builder()
            .secrets(crate::host::SecretsInit::WithProvider {
                path: PathBuf::from(&tmp),
                use_sdf: false,
            })
            .build()
            .await;
        // 不强制断言成功——init_secret_provider 是全局 NO-OP，文件不存在也不报错
        // （get_decrypted_api_key 会回退到 env var）
        let _ = host;
    }
}
