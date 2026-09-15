use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli::config::FileConfig;
use crate::cli::{
    build_compression_pipeline, build_llm_provider, resolve_effective_context_window, CliConfig,
    CliEventSink,
};

use super::attach::run_with_attach;
use super::skills::{handle_skill_command, resolve_skills_config_from_file, SkillCommands};

use clap::Parser;
use serde_json::Value;
use xiaoo_api::events::LoopEventSink;
use xiaoo_shared::backend::ProcessGroupCleanupGuard;
use xiaoo_shared::gateway::{
    session_record::SubagentRoleRecord, AppBootstrap, AppTurnRequest, GatewayEntryContext,
    HostedSessionRuntimeConfig, HostedSessionRuntimeResolver, InMemorySessionStore,
    LlmRuntimeConfig, McpMemoryAutomation, SessionRuntimeBindings, SessionRuntimeDescriptor,
    SessionRuntimeResolver, SessionStore,
};

use xiaoo_api::chat::AgentId;
use xiaoo_api::chat::ReasoningEffort;
use xiaoo_api::chat::{FeatureFlags, TokenBudgetConfig};
use xiaoo_api::chat::{HookerDefaultMode, HookerRegistryConfig};

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/cli_default_system_prompt.txt");

#[derive(Parser)]
#[command(name = "xiaoo --cli", about = "XiaoO AgentLoop CLI")]
struct Args {
    /// Path to config file (default: ~/.config/xiaoo/config.toml)
    #[arg(long, global = true)]
    config: Option<String>,

    /// Path to standard MCP JSON config (default discovery uses .mcp.json)
    #[arg(long, global = true)]
    mcp_config: Option<PathBuf>,

    /// Show intermediate results (turns, tool calls, tokens)
    #[arg(long, global = true)]
    debug: bool,

    /// Show version number
    #[arg(short = 'v', long = "version", global = true, action = clap::ArgAction::SetTrue)]
    version: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Run a single prompt through the AgentLoop
    Run {
        /// The prompt to send to the agent
        #[arg(short, long, num_args = 1..)]
        prompt: Vec<String>,

        /// LLM provider (overrides config file)
        #[arg(long)]
        provider: Option<String>,

        /// Model name (overrides config file)
        #[arg(long)]
        model: Option<String>,

        /// API key (overrides config file / env)
        #[arg(long)]
        api_key: Option<String>,

        /// Custom API base URL (overrides config file)
        #[arg(long)]
        api_base: Option<String>,

        /// System prompt
        #[arg(
            long,
            default_value_t = DEFAULT_SYSTEM_PROMPT.trim_end_matches(['\r', '\n']).to_string()
        )]
        system: String,

        /// Max turns per agent loop invocation
        #[arg(long, default_value_t = 10)]
        max_turns: u32,

        /// Disable tool execution
        #[arg(long)]
        no_tools: bool,

        /// Restrict to a comma-separated allowlist of tools
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,

        /// Reasoning effort: off, high, or max
        #[arg(long, value_parser = clap::value_parser!(ReasoningEffort))]
        reasoning_effort: Option<ReasoningEffort>,

        /// Output format for results
        #[arg(long, value_parser = clap::value_parser!(OutputFormat), default_value = "default")]
        format: OutputFormat,

        /// Human-readable session title
        #[arg(long)]
        title: Option<String>,

        /// Resume an existing session by ID
        #[arg(short, long)]
        session: Option<String>,

        /// Agent ID to use for this run
        #[arg(long)]
        agent: Option<String>,

        /// Attach to a running daemon at the given URL instead of running locally
        #[arg(long)]
        attach: Option<String>,
    },
    /// Start a local daemon server
    Serve {
        /// Port to listen on
        #[arg(long, default_value_t = 4096)]
        port: u16,

        /// Hostname to bind
        #[arg(long, default_value_t = String::from("127.0.0.1"))]
        hostname: String,
    },
    /// Export a session transcript from a running daemon
    Export {
        /// ID of the session to export
        session_id: String,
        /// Port of the running daemon
        #[arg(long, default_value = "4096")]
        port: u16,
        /// Optional client id for lease verification (when the daemon enforces session leases)
        #[arg(long)]
        client_id: Option<String>,
    },
    /// Inspect resolved configuration and internal state
    Debug {
        #[command(subcommand)]
        command: DebugCommands,
    },
    /// Manage skills
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },
}

#[derive(clap::Subcommand)]
enum DebugCommands {
    /// Show resolved configuration
    Config,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
pub(super) enum OutputFormat {
    /// Human-readable text output
    Default,
    /// Machine-readable JSON output (one event object per line)
    Json,
}

pub async fn run_cli_from_args<I, T>(args: I)
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let _cleanup_guard = ProcessGroupCleanupGuard;

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    let args = Args::parse_from(args);
    let debug = args.debug;
    let config_path = FileConfig::resolve_path(args.config.as_deref());
    let mcp_config_path = args.mcp_config;

    if args.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    match args.command {
        None => {
            eprintln!("error: 'xiaoo' requires a subcommand but one was not provided");
            eprintln!("  [subcommands: run, serve, export, debug, skill, help]");
            std::process::exit(1);
        }
        Some(Command::Run {
            prompt,
            provider,
            model,
            api_key,
            api_base,
            system,
            max_turns,
            no_tools,
            tools,
            reasoning_effort,
            format,
            title,
            session,
            agent,
            attach,
        }) => {
            let prompt = prompt.join(" ");
            if let Some(path) = config_path.as_ref() {
                if let Err(error) = xiaoo_shared::llm_secrets::inject_llm_secrets_into_env(path) {
                    eprintln!(
                        "Failed to initialize LLM secrets from {}: {}",
                        path.display(),
                        error
                    );
                    std::process::exit(1);
                }
            }
            let file_cfg = config_path
                .as_ref()
                .map(|path| FileConfig::load_from_path(path, debug))
                .unwrap_or_default();
            let llm = file_cfg.llm.as_ref();

            let provider = provider
                .or_else(|| llm.and_then(|l| l.provider.clone()))
                .unwrap_or_else(|| "anthropic".into());
            let model = model
                .or_else(|| llm.and_then(|l| l.model.clone()))
                .unwrap_or_else(|| "claude-sonnet-4-20250514".into());
            let api_key = api_key.or_else(|| file_cfg.resolve_api_key());
            let api_key_env = llm.and_then(|l| l.api_key_env.clone());
            let api_base = api_base.or_else(|| llm.and_then(|l| l.api_base.clone()));
            let reasoning_effort = reasoning_effort.unwrap_or_default();

            let skills_config = resolve_skills_config_from_file(&file_cfg);
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let default_toml_source = Path::new("config.toml");
            let mcp_servers = match file_cfg.resolve_mcp_servers(
                mcp_config_path.as_deref(),
                &workspace,
                dirs::home_dir().as_deref(),
                config_path.as_deref().unwrap_or(default_toml_source),
            ) {
                Ok(servers) => servers,
                Err(error) => {
                    eprintln!("Failed to load MCP config: {error}");
                    std::process::exit(1);
                }
            };

            let config = CliConfig {
                provider,
                model,
                api_key,
                api_key_env,
                api_base,
                trace: file_cfg
                    .trace
                    .clone()
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
                system_prompt: system,
                max_turns,
                enable_tools: !no_tools,
                visible_tools: tools.filter(|t| !t.is_empty()),
                reasoning_effort,
                kvcache_enabled: llm.and_then(|l| l.kvcache_enabled).unwrap_or(false),
                kvcache_debug_enabled: llm.and_then(|l| l.kvcache_debug_enabled).unwrap_or(false),
                compact: file_cfg.compact.unwrap_or_default(),
                hooker: file_cfg.hooker.clone().unwrap_or(HookerRegistryConfig {
                    default: HookerDefaultMode::None,
                    ..HookerRegistryConfig::default()
                }),
                operation_backend: file_cfg.operation_backend.clone(),
                skills_config,
                subagent: file_cfg.subagent.clone(),
                mcp_servers,
                memory_automation: file_cfg.memory_automation.clone(),
            };

            let session_title = title.or_else(|| generate_title_from_prompt(&prompt));

            run_once(
                config,
                prompt,
                debug,
                format,
                session_title,
                session,
                agent,
                attach,
            )
            .await;
        }
        Some(Command::Serve { port, hostname }) => {
            handle_serve_command(port, hostname).await;
        }
        Some(Command::Export {
            session_id,
            port,
            client_id,
        }) => {
            handle_export_command(session_id, port, client_id).await;
        }
        Some(Command::Debug { command }) => {
            handle_debug_command(command, config_path.as_ref(), debug);
        }
        Some(Command::Skill { command }) => {
            handle_skill_command(command);
        }
    }
}

async fn run_once(
    config: CliConfig,
    prompt: String,
    debug: bool,
    format: OutputFormat,
    title: Option<String>,
    session: Option<String>,
    agent: Option<String>,
    attach: Option<String>,
) {
    if debug {
        eprintln!(
            "[config] provider={}, model={}, max_turns={}, format={:?}",
            config.provider, config.model, config.max_turns, format
        );
        if let Some(title) = &title {
            eprintln!("[config] title={}", title);
        }
        if let Some(session) = &session {
            eprintln!("[config] session={}", session);
        }
        if let Some(agent) = &agent {
            eprintln!("[config] agent={}", agent);
        }
        if let Some(attach) = &attach {
            eprintln!("[config] attach={}", attach);
        }
    }

    if let Some(attach_url) = &attach {
        run_with_attach(attach_url, prompt, format, title, session, agent, debug).await;
        return;
    }

    // 1. LLM provider (shared with compression pipeline)
    let llm_provider = match build_llm_provider(
        &config,
        Some(agent.clone().unwrap_or_else(|| "defaultagent".into())),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to create LLM provider: {}", e);
            std::process::exit(1);
        }
    };

    // 2. Compression pipeline
    let compression_pipeline = match build_compression_pipeline(&config, &llm_provider) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to build compression pipeline: {}", e);
            std::process::exit(1);
        }
    };

    // 3. Session runtime config
    let total_budget = resolve_effective_context_window(&config, &llm_provider).await;
    let reserved_for_output = total_budget / 10;
    let reserved_for_system = total_budget / 20;

    let runtime_config = HostedSessionRuntimeConfig {
        descriptor: SessionRuntimeDescriptor {
            agent_id: AgentId(
                agent
                    .as_ref()
                    .map(|a| a.clone())
                    .unwrap_or_else(|| "defaultagent".into()),
            ),
            model: config.model.clone(),
            llm: Some(LlmRuntimeConfig {
                profile_id: None,
                provider: Some(config.provider.clone()),
                model: Some(config.model.clone()),
                api_base: config.api_base.clone(),
                api_key_env: config.api_key_env.clone(),
                api_key: None,
                reasoning_effort: Some(config.reasoning_effort),
            }),
            system_prompt: config.system_prompt.clone(),
            feature_flags: FeatureFlags {
                tool_execution: config.enable_tools,
                kvcache_enabled: config.kvcache_enabled,
                kvcache_debug_enabled: config.kvcache_debug_enabled,
                ..FeatureFlags::default()
            },
            token_budget: TokenBudgetConfig {
                total_budget,
                reserved_for_output,
                reserved_for_system,
                hard_limit_ratio: 0.9,
            },
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            max_turns: Some(config.max_turns),
            subagent_roles: config
                .subagent
                .iter()
                .map(|(role_id, cfg)| {
                    (
                        role_id.clone(),
                        SubagentRoleRecord {
                            role_id: role_id.clone(),
                            description: cfg.description.clone(),
                            prompt: cfg.prompt.clone(),
                            max_turns: cfg.max_turns,
                            tools: cfg.tools.clone(),
                        },
                    )
                })
                .collect(),
        },
        trace: config.trace.clone(),
        provider: config.provider.clone(),
        model: config.model.clone(),
        api_key: config.api_key.clone(),
        api_key_env: config.api_key_env.clone(),
        api_base: config.api_base.clone(),
        visible_tool_names: if !config.enable_tools {
            Some(Vec::new())
        } else {
            config.visible_tools.clone()
        },
        compression_pipeline: Some(compression_pipeline),
        llm_provider: Some(llm_provider),
        hooker: config.hooker.clone(),
        lsp_registry: None,
        operation_backend: config.operation_backend.clone(),
        skills_config: config.skills_config.clone(),
        subagent_roles: config
            .subagent
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    xiaoo_shared::gateway::SubagentRoleConfigEntry {
                        description: v.description.clone(),
                        prompt: v.prompt.clone(),
                        max_turns: v.max_turns,
                        tools: v.tools.clone(),
                    },
                )
            })
            .collect(),
        mcp_servers: config.mcp_servers.clone(),
        memory_automation: config.memory_automation.clone(),
    };

    // 4. Bindings (CliEventSink for debug output)
    let loop_event_sink: Option<Arc<dyn LoopEventSink>> =
        debug.then(|| Arc::new(CliEventSink::new()) as Arc<dyn LoopEventSink>);
    let bindings = SessionRuntimeBindings {
        loop_event_sink,
        tool_event_sink: None,
        interaction_handle: None,
        channel_file_sender: None,
        pending_user_messages: None,
        cancel_token: None,
    };

    // 5. Bootstrap gateway
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let memory_automation = match McpMemoryAutomation::connect(
        config.memory_automation.clone(),
        &config.mcp_servers,
    )
    .await
    {
        Ok(automation) => automation,
        Err(error) => {
            tracing::warn!(error = %error, "memory automation disabled after CLI startup error");
            None
        }
    };
    let memory_automation_for_shutdown = memory_automation.clone();
    let resolver: Arc<dyn SessionRuntimeResolver> =
        Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
    let deps = match AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
        store,
        resolver,
        config.hooker.clone(),
        Arc::new(xiaoo_shared::backend::BackendManager::new()),
        memory_automation,
        // No subagent interaction timeout for the local CLI/TUI entry.
        None,
    ) {
        Ok(d) => d,
        Err(e) => {
            if let Some(automation) = memory_automation_for_shutdown {
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    automation.close(),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        eprintln!("[warn] failed to close MCP memory automation: {error}")
                    }
                    Err(_) => eprintln!("[warn] MCP memory automation close timed out after 5 seconds"),
                }
            }
            eprintln!("Failed to bootstrap session: {}", e);
            std::process::exit(1);
        }
    };

    // 6. Turn request - use provided session ID or create new one
    let session_id = session.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext::cli(),
        channel: None,
        message_id: None,
        conversation_id: session_id.clone(),
        sender_id: "cli-user".into(),
        text: prompt.clone(),
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
        reasoning_effort: Some(config.reasoning_effort),
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        // One-shot CLI never opens a session, so it skips the attach-lease
        // protocol and carries no `client_id`. With `XIAOO_ENFORCE_LEASE=on`,
        // use the TUI instead.
        client_id: None,
    };

    // Print session info
    if debug || format == OutputFormat::Json {
        let session_info = serde_json::json!({
            "session_id": session_id,
            "title": title,
            "agent": agent,
        });
        if format == OutputFormat::Json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "type": "session_start",
                    "data": session_info
                }))
                .unwrap()
            );
            let _ = std::io::stdout().flush();
        } else if debug {
            eprintln!(
                "[session] {}",
                serde_json::to_string_pretty(&session_info).unwrap()
            );
        }
    }

    // 7. Run turn via gateway session service, then explicitly close the
    // session so SessionClosed lifecycle hookers fire in CLI mode as well.
    let turn_result = deps.session_service.run_turn(request).await;
    if let Err(err) = deps
        .session_control_plane
        .force_close_session(&session_id)
        .await
    {
        if format == OutputFormat::Json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "type": "error",
                    "data": {
                        "message": format!("failed to close session: {}", err)
                    }
                }))
                .unwrap()
            );
            let _ = std::io::stdout().flush();
        } else {
            eprintln!("[warn] failed to close session: {}", err);
        }
    }
    if let Some(automation) = memory_automation_for_shutdown {
        match tokio::time::timeout(std::time::Duration::from_secs(5), automation.close()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("[warn] failed to close MCP memory automation: {error}"),
            Err(_) => eprintln!("[warn] MCP memory automation close timed out after 5 seconds"),
        }
    }

    match turn_result {
        Ok(result) => {
            if format == OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "type": "response",
                        "data": {
                            "raw_reply": result.raw_reply,
                            "session_id": session_id,
                        }
                    }))
                    .unwrap()
                );
                let _ = std::io::stdout().flush();
            } else {
                if !result.raw_reply.is_empty() {
                    println!("{}", result.raw_reply);
                }
            }
        }
        Err(e) => {
            if format == OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "type": "error",
                        "data": {
                            "message": e.to_string()
                        }
                    }))
                    .unwrap()
                );
                let _ = std::io::stdout().flush();
            } else {
                eprintln!("[error] {}", e);
            }
            std::process::exit(1);
        }
    }
}

fn generate_title_from_prompt(prompt: &str) -> Option<String> {
    let words = prompt.split_whitespace().take(10).collect::<Vec<_>>();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

async fn handle_serve_command(port: u16, hostname: String) {
    eprintln!("Starting xiaoo daemon server on {}:{}", hostname, port);
    eprintln!("Use 'xiaoo-daemon' binary directly for full daemon functionality");
    let status = std::process::Command::new("xiaoo-daemon")
        .args(["--port", &port.to_string(), "--host", &hostname])
        .status();

    match status {
        Ok(s) if s.success() => std::process::exit(0),
        Ok(s) => {
            eprintln!("xiaoo-daemon exited with status: {}", s);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Failed to start xiaoo-daemon: {}", e);
            eprintln!("Make sure 'xiaoo-daemon' binary is installed");
            std::process::exit(1);
        }
    }
}

fn handle_debug_command(command: DebugCommands, config_path: Option<&PathBuf>, debug: bool) {
    match command {
        DebugCommands::Config => {
            let file_cfg = config_path
                .map(|path| FileConfig::load_from_path(path, debug))
                .unwrap_or_default();

            let mut config_json = serde_json::Map::new();
            config_json.insert(
                "$schema".to_string(),
                Value::String("https://xiaoo.ai/config.json".to_string()),
            );

            if let Some(llm) = &file_cfg.llm {
                let provider = llm.provider.as_deref().unwrap_or("openai");
                let model = llm.model.as_deref().unwrap_or("");
                config_json.insert(
                    "model".to_string(),
                    Value::String(format!("{}/{}", provider, model)),
                );
            }

            println!(
                "{}",
                serde_json::to_string_pretty(&Value::Object(config_json)).unwrap()
            );
        }
    }
}
async fn handle_export_command(session_id: String, port: u16, client_id: Option<String>) {
    let url = format!(
        "http://127.0.0.1:{}/api/v1/runtimes/export/{}",
        port, session_id
    );

    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    if let Some(cid) = &client_id {
        req = req.query(&[("client_id", cid)]);
    }
    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await;
            if status.is_success() {
                match text {
                    Ok(body) if !body.is_empty() => println!("{}", body),
                    Ok(_) => {
                        eprintln!("Error: Empty response exporting session '{}'", session_id);
                        eprintln!("Make sure xiaoo-daemon is running on port {}", port);
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Error: Failed to read export response body: {}", e);
                        eprintln!("Make sure xiaoo-daemon is running on port {}", port);
                        std::process::exit(1);
                    }
                }
            } else {
                eprintln!("Error: Failed to export session '{}'", session_id);
                eprintln!("Details: HTTP {}", status.as_u16());
                if let Ok(body) = &text {
                    if !body.is_empty() {
                        eprintln!("Response: {}", body);
                    }
                }
                eprintln!("Make sure xiaoo-daemon is running on port {}", port);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: Failed to call export API: {}", e);
            eprintln!("Make sure xiaoo-daemon is running on port {}", port);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/cli/entry_test.rs"]
mod tests;
