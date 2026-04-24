use std::path::PathBuf;
use std::sync::Arc;

use agent_contracts::SkillRegistry;
use clap::Parser;
use serde_json::Value;
use skill::audit::{audit_skill_directory, SkillAuditOptions};
use skill::registry::FileSkillRegistry;
use skill::types::config::SkillsConfig;
use xiaoo_app::cli::config::FileConfig;
use xiaoo_app::cli::{
    build_compression_pipeline, build_llm_provider, resolve_effective_context_window, CliConfig,
    CliEventSink,
};
use xiaoo_app::gateway::{
    AppBootstrap, AppTurnRequest, GatewayEntryContext, HostedSessionRuntimeConfig,
    HostedSessionRuntimeResolver, InMemorySessionStore, SessionRuntimeBindings,
    SessionRuntimeDescriptor, SessionRuntimeResolver, SessionStore,
};

use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/cli_default_system_prompt.txt");

#[derive(Parser)]
#[command(name = "xiaoo", about = "XiaoO AgentLoop CLI")]
struct Args {
    /// Path to config file (default: ~/.config/xiaoo/config.toml)
    #[arg(long, global = true)]
    config: Option<String>,

    /// Show intermediate results (turns, tool calls, tokens)
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Run a single prompt through the AgentLoop
    Run {
        /// The prompt to send to the agent
        #[arg(short, long)]
        prompt: String,

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
    },
    /// Manage skills
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },
}

#[derive(clap::Subcommand)]
enum SkillCommands {
    /// List all installed skills
    List,
    /// Show details of a specific skill
    Show { name: String },
    /// Run security audit on a skill directory
    Audit { path: String },
    /// Install a skill from a local directory or git URL
    Install { source: String },
    /// Remove an installed skill
    Remove { name: String },
}

#[tokio::main]
async fn main() {
    // Initialize tracing (reads RUST_LOG env)
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let debug = args.debug;
    let file_cfg = FileConfig::load(args.config.as_deref(), debug);

    match args.command {
        Command::Run {
            prompt,
            provider,
            model,
            api_key,
            api_base,
            system,
            max_turns,
            no_tools,
        } => {
            let llm = file_cfg.llm.as_ref();

            let provider = provider
                .or_else(|| llm.and_then(|l| l.provider.clone()))
                .unwrap_or_else(|| "anthropic".into());
            let model = model
                .or_else(|| llm.and_then(|l| l.model.clone()))
                .unwrap_or_else(|| "claude-sonnet-4-20250514".into());
            let api_key = api_key.or_else(|| file_cfg.resolve_api_key());
            let api_base = api_base.or_else(|| llm.and_then(|l| l.api_base.clone()));
            let context_window = llm.and_then(|l| l.context_window);

            let config = CliConfig {
                provider,
                model,
                api_key,
                api_base,
                trace: file_cfg
                    .trace
                    .clone()
                    .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
                system_prompt: system,
                max_turns,
                enable_tools: !no_tools,
                context_window,
                compact: file_cfg.compact.unwrap_or_default(),
                hooker: file_cfg.hooker.clone().unwrap_or(HookerRegistryConfig {
                    default: HookerDefaultMode::None,
                    ..HookerRegistryConfig::default()
                }),
                operation_backend: file_cfg.operation_backend.clone(),
            };

            run_once(config, prompt, debug).await;
        }
        Command::Skill { command } => {
            handle_skill_command(command);
        }
    }
}

fn default_skills_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiaoo").join("skills"))
        .unwrap_or_else(|| PathBuf::from(".xiaoo/skills"))
}

fn default_skills_config() -> SkillsConfig {
    SkillsConfig {
        skills_dirs: vec![default_skills_dir()],
        ..SkillsConfig::default()
    }
}

fn handle_skill_command(command: SkillCommands) {
    match command {
        SkillCommands::List => {
            let registry = FileSkillRegistry::new(&default_skills_config());
            let skills = registry.list_skills();
            if skills.is_empty() {
                println!("No skills installed.");
                println!("  Skills directory: {}", default_skills_dir().display());
                return;
            }
            println!("{:<20} {}", "NAME", "DESCRIPTION");
            println!("{:<20} {}", "----", "-----------");
            for s in &skills {
                println!("{:<20} {}", s.skill_id, s.description);
            }
            println!("\n{} skill(s) found.", skills.len());
        }
        SkillCommands::Show { name } => {
            let registry = FileSkillRegistry::new(&default_skills_config());
            match registry.get_skill(&name) {
                Some(spec) => {
                    println!("Skill: {}", spec.skill_id());
                    println!("Description: {}", spec.description());
                    if !spec.arguments().is_empty() {
                        println!("Arguments: {}", spec.arguments().join(", "));
                    }
                    if let Some(hint) = spec.argument_hint() {
                        println!("Argument hint: {}", hint);
                    }
                    println!("Context: {:?}", spec.context());
                    println!("User invocable: {}", spec.user_invocable());
                    if let Some(loc) = spec.location() {
                        println!("Location: {}", loc.display());
                    }
                    println!("\n--- Prompt ---\n{}", spec.full_prompt());
                }
                None => {
                    eprintln!("Skill '{}' not found.", name);
                    std::process::exit(1);
                }
            }
        }
        SkillCommands::Audit { path } => {
            let dir = PathBuf::from(&path);
            if !dir.is_dir() {
                eprintln!("Not a directory: {}", path);
                std::process::exit(1);
            }
            let report = audit_skill_directory(&dir, &SkillAuditOptions::default());
            println!("Audited: {}", dir.display());
            println!("Files scanned: {}", report.files_scanned);
            if report.is_clean() {
                println!("Result: CLEAN");
            } else {
                println!("Result: {} issue(s) found:", report.findings.len());
                for (i, f) in report.findings.iter().enumerate() {
                    println!("  {}. {}", i + 1, f);
                }
                std::process::exit(1);
            }
        }
        SkillCommands::Install { source } => {
            let is_git = source.ends_with(".git")
                || source.starts_with("https://")
                || source.starts_with("http://")
                || source.starts_with("git@")
                || source.starts_with("file://");

            let src_dir = if is_git {
                let repo_name = extract_repo_name(&source);
                let tmp = std::env::temp_dir().join(&repo_name);
                let _ = std::fs::remove_dir_all(&tmp);
                println!("Cloning {} ...", source);
                let status = std::process::Command::new("git")
                    .args([
                        "clone",
                        "--depth",
                        "1",
                        &source,
                        tmp.to_str().unwrap_or("."),
                    ])
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    Ok(s) => {
                        eprintln!("git clone failed: {}", s);
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Failed to run git: {}", e);
                        std::process::exit(1);
                    }
                }
                let _ = std::fs::remove_dir_all(tmp.join(".git"));
                tmp
            } else {
                let p = PathBuf::from(&source);
                if !p.is_dir() {
                    eprintln!("Not a directory: {}", source);
                    std::process::exit(1);
                }
                p
            };

            let skill_name = src_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .strip_suffix(".git")
                .unwrap_or(
                    src_dir
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown"),
                )
                .to_string();

            if skill_name.contains("..") || skill_name.contains('/') || skill_name.contains('\\') {
                eprintln!("Invalid skill name: {}", skill_name);
                std::process::exit(1);
            }

            let dest = default_skills_dir().join(&skill_name);
            if dest.exists() {
                eprintln!(
                    "Skill '{}' already installed at {}",
                    skill_name,
                    dest.display()
                );
                if is_git {
                    let _ = std::fs::remove_dir_all(&src_dir);
                }
                std::process::exit(1);
            }

            // Audit is currently disabled by default; use `xiaoo skill audit <path>` for manual checks.

            if let Err(e) = copy_dir_recursive(&src_dir, &dest) {
                eprintln!("Failed to install: {}", e);
                if is_git {
                    let _ = std::fs::remove_dir_all(&src_dir);
                }
                std::process::exit(1);
            }
            if is_git {
                let _ = std::fs::remove_dir_all(&src_dir);
            }
            println!("Installed skill '{}' to {}", skill_name, dest.display());
        }
        SkillCommands::Remove { name } => {
            if name.contains("..") || name.contains('/') || name.contains('\\') {
                eprintln!("Invalid skill name: {}", name);
                std::process::exit(1);
            }
            let dir = default_skills_dir().join(&name);
            if !dir.is_dir() {
                eprintln!("Skill '{}' not found at {}", name, dir.display());
                std::process::exit(1);
            }
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                eprintln!("Failed to remove: {}", e);
                std::process::exit(1);
            }
            println!("Removed skill '{}'.", name);
        }
    }
}

fn extract_repo_name(url: &str) -> String {
    let name = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url);
    let name = name.rsplit(':').next().unwrap_or(name);
    let name = name.rsplit('/').next().unwrap_or(name);
    let name = name.strip_suffix(".git").unwrap_or(name);
    if name.is_empty() {
        format!("skill-{}", std::process::id())
    } else {
        name.to_string()
    }
}

fn copy_dir_recursive(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), dst)?;
        }
    }
    Ok(())
}

async fn run_once(config: CliConfig, prompt: String, debug: bool) {
    if debug {
        eprintln!(
            "[config] provider={}, model={}, max_turns={}",
            config.provider, config.model, config.max_turns
        );
    }

    // 1. LLM provider (shared with compression pipeline)
    let llm_provider = match build_llm_provider(&config, Some("defaultagent".into())) {
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
            agent_id: AgentId("defaultagent".into()),
            model: config.model.clone(),
            system_prompt: config.system_prompt.clone(),
            feature_flags: FeatureFlags {
                tool_execution: config.enable_tools,
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
        },
        trace: config.trace.clone(),
        provider: config.provider.clone(),
        model: config.model.clone(),
        api_key: config.api_key.clone(),
        api_key_env: None,
        api_base: config.api_base.clone(),
        visible_tool_names: if config.enable_tools {
            None
        } else {
            Some(Vec::new())
        },
        compression_pipeline: Some(compression_pipeline),
        llm_provider: Some(llm_provider),
        hooker: config.hooker.clone(),
        lsp_service: None,
        operation_backend: config.operation_backend.clone(),
    };

    // 4. Bindings (CliEventSink for debug output)
    let bindings = SessionRuntimeBindings {
        loop_event_sink: Some(Arc::new(CliEventSink { debug })),
        tool_event_sink: None,
        interaction_handle: None,
        channel_file_sender: None,
    };

    // 5. Bootstrap gateway
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let resolver: Arc<dyn SessionRuntimeResolver> =
        Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
    let deps = match AppBootstrap::from_session_components_with_hooks(
        store,
        resolver,
        config.hooker.clone(),
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to bootstrap session: {}", e);
            std::process::exit(1);
        }
    };

    // 6. Turn request
    let session_id = uuid::Uuid::new_v4().to_string();
    let request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext::cli(),
        channel: None,
        message_id: None,
        conversation_id: session_id.clone(),
        sender_id: "cli-user".into(),
        text: prompt,
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
    };

    // 7. Run turn via gateway session service, then explicitly close the
    // session so SessionClosed lifecycle hookers fire in CLI mode as well.
    let turn_result = deps.session_service.run_turn(request).await;
    if let Err(err) = deps
        .session_control_plane
        .force_close_session(&session_id)
        .await
    {
        eprintln!("[warn] failed to close session: {}", err);
    }

    match turn_result {
        Ok(result) => {
            if !result.raw_reply.is_empty() {
                println!("{}", result.raw_reply);
            }
        }
        Err(e) => {
            eprintln!("[error] {}", e);
            std::process::exit(1);
        }
    }
}
