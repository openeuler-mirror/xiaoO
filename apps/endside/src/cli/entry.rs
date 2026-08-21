use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::cli::config::FileConfig;
use crate::cli::{
    build_compression_pipeline, build_llm_provider, resolve_effective_context_window, CliConfig,
    CliEventSink,
};
use agent_contracts::{LoopEventSink, SkillRegistry};
use clap::Parser;
use futures_util::StreamExt;
use operation_backend::process_group::ProcessGroupCleanupGuard;
use serde_json::Value;
use skill::audit::{audit_skill_directory, SkillAuditOptions};
use skill::registry::FileSkillRegistry;
use skill::types::config::SkillsConfig;
use xiaoo_shared::gateway::{
    session_record::SubagentRoleRecord, AppBootstrap, AppTurnRequest, GatewayEntryContext,
    HostedSessionRuntimeConfig, HostedSessionRuntimeResolver, InMemorySessionStore,
    LlmRuntimeConfig, McpMemoryAutomation, SessionDetachRequest, SessionOpenRequest,
    SessionRuntimeBindings, SessionRuntimeDescriptor, SessionRuntimeResolver, SessionStore,
};

use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::{HookerDefaultMode, HookerRegistryConfig};
use agent_types::ReasoningEffort;

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
enum OutputFormat {
    /// Human-readable text output
    Default,
    /// Machine-readable JSON output (one event object per line)
    Json,
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
                if let Err(error) = xiaoo_shared::llm_secrets::inject_llm_secrets_into_env(path)
                {
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

fn resolve_skills_config_from_file(file_cfg: &FileConfig) -> skill::SkillsConfig {
    // Build complete skills_dirs with four levels
    let mut skills_dirs = Vec::new();

    // Priority 1: Project level (highest)
    skills_dirs.push(PathBuf::from(".xiaoo/skills"));

    // Priority 2: Config file user dirs (medium)
    if let Some(skills_section) = file_cfg.skills.as_ref() {
        if let Some(extra_dirs) = skills_section.dirs.as_ref() {
            for dir in extra_dirs {
                let path = PathBuf::from(dir);
                // Avoid duplicates with default dirs
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
    }

    // Priority 3: User level
    if let Some(home) = dirs::home_dir() {
        skills_dirs.push(home.join(".xiaoo").join("skills"));
    }

    // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
    skills_dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

    skill::SkillsConfig {
        skills_dirs,
        allow_scripts: file_cfg
            .skills
            .as_ref()
            .and_then(|s| s.allow_scripts)
            .unwrap_or(false),
        ..skill::SkillsConfig::default()
    }
}

fn resolve_all_skills_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Priority 1: Project level (highest)
    dirs.push(PathBuf::from(".xiaoo/skills"));

    // Priority 2: Config file user dirs (medium)
    let file_cfg = FileConfig::load(None, false);
    if let Some(skills) = file_cfg.skills.as_ref() {
        if let Some(extra_dirs) = skills.dirs.as_ref() {
            for dir in extra_dirs {
                let path = PathBuf::from(dir);
                // Avoid duplicates with project/user/system dirs
                let dir_str = path.to_string_lossy();
                if dir_str != ".xiaoo/skills"
                    && !dir_str.ends_with("/.xiaoo/skills")
                    && !dir_str.ends_with("\\.xiaoo\\skills")
                    && dir_str != "/usr/lib/.xiaoo/skills"
                {
                    dirs.push(path);
                }
            }
        }
    }

    // Priority 3: User level
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".xiaoo").join("skills"));
    }

    // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
    dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

    dirs
}

fn build_skills_config() -> SkillsConfig {
    let skills_dirs = resolve_all_skills_dirs();

    // Get allow_scripts from config file
    let file_cfg = FileConfig::load(None, false);
    let allow_scripts = file_cfg
        .skills
        .as_ref()
        .and_then(|s| s.allow_scripts)
        .unwrap_or(false);

    SkillsConfig {
        skills_dirs,
        allow_scripts,
        ..SkillsConfig::default()
    }
}

fn project_skills_dir() -> PathBuf {
    PathBuf::from(".xiaoo/skills")
}

fn user_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".xiaoo").join("skills"))
}

fn system_skills_dir() -> PathBuf {
    PathBuf::from("/usr/lib/.xiaoo/skills")
}

fn default_skills_config() -> SkillsConfig {
    build_skills_config()
}

fn handle_skill_command(command: SkillCommands) {
    match command {
        SkillCommands::List => {
            let registry = FileSkillRegistry::new(&default_skills_config());
            let skills = registry.list_skills();
            if skills.is_empty() {
                println!("No skills installed.");
                let dirs = resolve_all_skills_dirs();
                for d in &dirs {
                    println!("  Skills directory: {}", d.display());
                }
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

            // Extract skill name first (before cloning/downloading)
            let skill_name = if is_git {
                extract_repo_name(&source)
            } else {
                let p = PathBuf::from(&source);
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .strip_suffix(".git")
                    .unwrap_or(p.file_name().and_then(|n| n.to_str()).unwrap_or("unknown"))
                    .to_string()
            };

            if skill_name.contains("..") || skill_name.contains('/') || skill_name.contains('\\') {
                eprintln!("Invalid skill name: {}", skill_name);
                std::process::exit(1);
            }

            // Check all skill directories for existing skill BEFORE cloning
            let project_dest = project_skills_dir().join(&skill_name);
            let user_dest = user_skills_dir().as_ref().map(|d| d.join(&skill_name));
            let system_dest = system_skills_dir().join(&skill_name);

            // Check config file directories
            let config_dests = {
                let file_cfg = FileConfig::load(None, false);
                file_cfg
                    .skills
                    .as_ref()
                    .and_then(|s| s.dirs.as_ref())
                    .map(|dirs| {
                        dirs.iter()
                            .map(|d| PathBuf::from(d).join(&skill_name))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };

            // Check project level first (highest priority)
            if project_dest.exists() {
                eprintln!(
                    "Skill '{}' already installed at {} (project level, highest priority)",
                    skill_name,
                    project_dest.display()
                );
                std::process::exit(1);
            }

            // Check config file directories (medium priority)
            for config_dest in &config_dests {
                if config_dest.exists() {
                    eprintln!(
                        "Skill '{}' already installed at {} (config directory, medium priority)",
                        skill_name,
                        config_dest.display()
                    );
                    std::process::exit(1);
                }
            }

            // Check user level
            if let Some(ref user_d) = user_dest {
                if user_d.exists() {
                    eprintln!(
                        "Skill '{}' already installed at {} (user level)",
                        skill_name,
                        user_d.display()
                    );
                    std::process::exit(1);
                }
            }

            // Check system level (lowest priority, for built-in skills only)
            if system_dest.exists() {
                eprintln!(
                    "Skill '{}' already installed at {} (system level, built-in skill)",
                    skill_name,
                    system_dest.display()
                );
                std::process::exit(1);
            }

            // Now clone/copy the source
            let src_dir = if is_git {
                let tmp = std::env::temp_dir().join(&skill_name);
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

            // Validate that source directory contains a valid skill (SKILL.md or SKILL.toml)
            let has_manifest =
                src_dir.join("SKILL.md").exists() || src_dir.join("SKILL.toml").exists();
            if !has_manifest {
                eprintln!("Error: Source directory is not a valid skill directory.");
                eprintln!("A valid skill directory must contain either SKILL.md or SKILL.toml.");
                if is_git {
                    let _ = std::fs::remove_dir_all(&src_dir);
                }
                std::process::exit(1);
            }

            // Install to user directory by default
            // Users can manually copy to project level or config directories to override
            // System level (/usr/lib/.xiaoo/skills) is reserved for built-in skills only
            let dest = user_dest.unwrap_or_else(|| project_skills_dir().join(&skill_name));

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

            let project_dir = project_skills_dir().join(&name);
            let user_dir = user_skills_dir().as_ref().map(|d| d.join(&name));
            let system_dir = system_skills_dir().join(&name);

            // Get config file directories
            let config_dirs = {
                let file_cfg = FileConfig::load(None, false);
                file_cfg
                    .skills
                    .as_ref()
                    .and_then(|s| s.dirs.as_ref())
                    .map(|dirs| {
                        dirs.iter()
                            .map(|d| PathBuf::from(d).join(&name))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };

            // Priority: remove from highest priority level first
            // 1. Project level (highest)
            if project_dir.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&project_dir) {
                    eprintln!("Failed to remove from project: {}", e);
                    std::process::exit(1);
                }
                println!(
                    "Removed skill '{}' from {} (project level, highest priority).",
                    name,
                    project_dir.display()
                );

                // Warn if config dirs still exist
                for config_dir in &config_dirs {
                    if config_dir.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (config directory).",
                            name,
                            config_dir.display()
                        );
                    }
                }

                // Warn if user level still exists
                if let Some(ref user_d) = user_dir {
                    if user_d.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (user level).",
                            name,
                            user_d.display()
                        );
                    }
                }

                // Warn if system level still exists
                if system_dir.is_dir() {
                    eprintln!(
                        "Warning: Skill '{}' still exists at {} (system level, built-in skill).",
                        name,
                        system_dir.display()
                    );
                }
                return;
            }

            // 2. Config directories (medium)
            for config_dir in &config_dirs {
                if config_dir.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(config_dir) {
                        eprintln!("Failed to remove from config directory: {}", e);
                        std::process::exit(1);
                    }
                    println!(
                        "Removed skill '{}' from {} (config directory).",
                        name,
                        config_dir.display()
                    );

                    // Warn if other config dirs or user/system still exist
                    for other_config_dir in &config_dirs {
                        if other_config_dir != config_dir && other_config_dir.is_dir() {
                            eprintln!(
                                "Warning: Skill '{}' still exists at {} (other config directory).",
                                name,
                                other_config_dir.display()
                            );
                        }
                    }

                    if let Some(ref user_d) = user_dir {
                        if user_d.is_dir() {
                            eprintln!(
                                "Warning: Skill '{}' still exists at {} (user level).",
                                name,
                                user_d.display()
                            );
                        }
                    }

                    if system_dir.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (system level, built-in skill).",
                            name,
                            system_dir.display()
                        );
                    }
                    return;
                }
            }

            // 3. User level
            if let Some(ref user_d) = user_dir {
                if user_d.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(user_d) {
                        eprintln!("Failed to remove from user directory: {}", e);
                        std::process::exit(1);
                    }
                    println!(
                        "Removed skill '{}' from {} (user level).",
                        name,
                        user_d.display()
                    );

                    // Warn if system level still exists
                    if system_dir.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (system level, built-in skill).",
                            name,
                            system_dir.display()
                        );
                    }
                    return;
                }
            }

            // 4. System level (built-in skills only - requires root privileges to remove)
            if system_dir.is_dir() {
                eprintln!(
                    "Skill '{}' is a built-in skill at {} (system level).",
                    name,
                    system_dir.display()
                );
                eprintln!("Built-in skills require root privileges to remove.");
                eprintln!("To remove: sudo rm -rf {}", system_dir.display());
                std::process::exit(1);
            }

            // Skill not found anywhere
            eprintln!("Skill '{}' not found in any skills directory.", name);
            eprintln!("Checked directories:");
            eprintln!(
                "  - {} (project level, highest priority)",
                project_dir.display()
            );
            for config_dir in &config_dirs {
                eprintln!("  - {} (config directory)", config_dir.display());
            }
            if let Some(ref user_d) = user_dir {
                eprintln!("  - {} (user level)", user_d.display());
            }
            eprintln!(
                "  - {} (system level, built-in skills)",
                system_dir.display()
            );
            std::process::exit(1);
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
    reject_nested_copy(src, dest)?;
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

fn reject_nested_copy(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    let src = src.canonicalize()?;
    let dest_parent = dest.parent().unwrap_or_else(|| std::path::Path::new("."));
    let dest_name = dest.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination has no name")
    })?;
    let dest_abs = dest_parent
        .canonicalize()
        .unwrap_or_else(|_| dest_parent.to_path_buf())
        .join(dest_name);

    if dest_abs.starts_with(&src) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination must not be inside source directory",
        ));
    }
    Ok(())
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
                provider: Some(config.provider.clone()),
                model: Some(config.model.clone()),
                api_base: config.api_base.clone(),
                api_key_env: config.api_key_env.clone(),
                api_key: None,
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
        reasoning_effort: config.reasoning_effort,
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

async fn run_with_attach(
    url: &str,
    prompt: String,
    format: OutputFormat,
    title: Option<String>,
    session: Option<String>,
    agent: Option<String>,
    debug: bool,
) {
    let base_url = url.trim_end_matches('/').to_string();
    if debug {
        eprintln!("Attaching to daemon at: {base_url}");
    }

    // Optional bearer token so attach also works against daemons that enable
    // HTTP bearer auth (mirrors the TUI's resolve_bearer_token).
    let bearer_token = std::env::var("XIAOO_DAEMON_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // This process's identity for the daemon's attach-lease table.
    let client_id = format!("cli-{}", std::process::id());
    let session_id = session.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let is_json = format == OutputFormat::Json;

    if is_json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "type": "session_start",
                "data": {
                    "session_id": &session_id,
                    "title": &title,
                    "agent": &agent,
                }
            }))
            .unwrap()
        );
        let _ = std::io::stdout().flush();
    }

    // 1. Open (or re-open) the session so this process holds the lease --
    //    required before submitting a turn when the daemon enforces lease
    //    ownership (`XIAOO_ENFORCE_LEASE=on`).
    let open_request = SessionOpenRequest {
        session_id: session_id.clone(),
        conversation_id: session_id.clone(),
        sender_id: "cli-user".to_string(),
        entry: GatewayEntryContext::cli(),
        channel: None,
        channel_instance_id: None,
        llm: None,
        workspace: None,
        skills: None,
        client_id: Some(client_id.clone()),
        client_pid: Some(std::process::id()),
        client_hostname: None,
    };
    let open_url = format!("{base_url}/api/v1/runtimes/open");
    let mut open_req = client.post(&open_url).json(&open_request);
    if let Some(token) = bearer_token.as_ref() {
        open_req = open_req.bearer_auth(token);
    }
    match open_req.send().await {
        Ok(resp) if !resp.status().is_success() => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            attach_fail(
                format!("session open failed: HTTP {status} {body}"),
                is_json,
            );
        }
        Ok(_) => {}
        Err(error) => attach_fail(format!("failed to connect to daemon: {error}"), is_json),
    }

    // 2. Submit the turn; the daemon replies with an SSE event stream.
    let turn_request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext::cli(),
        channel: None,
        message_id: None,
        conversation_id: session_id.clone(),
        sender_id: "cli-user".to_string(),
        text: prompt,
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
        reasoning_effort: ReasoningEffort::default(),
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        client_id: Some(client_id.clone()),
    };
    let input_url = format!("{base_url}/api/v1/runtimes/input");
    let mut input_req = client.post(&input_url).json(&turn_request);
    if let Some(token) = bearer_token.as_ref() {
        input_req = input_req.bearer_auth(token);
    }
    let response = match input_req.send().await {
        Ok(response) => response,
        Err(error) => attach_fail(format!("failed to submit turn: {error}"), is_json),
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        attach_fail(
            format!("turn submission failed: HTTP {status} {body}"),
            is_json,
        );
    }

    // 3. Consume the SSE event stream emitted by /api/v1/runtimes/input.
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut printed_any_text = false;
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => attach_fail(format!("stream read error: {error}"), is_json),
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(frame) = take_sse_frame(&mut buffer) {
            let Some(event) = parse_sse_event(&frame) else {
                continue;
            };
            let typ = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match typ {
                "text_delta" => {
                    if is_json {
                        println!("{}", serde_json::to_string(&event).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    } else if let Some(delta) = event.get("delta").and_then(|v| v.as_str()) {
                        print!("{delta}");
                        let _ = std::io::stdout().flush();
                        printed_any_text = true;
                    }
                }
                "done" => {
                    saw_done = true;
                    if is_json {
                        println!("{}", serde_json::to_string(&event).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    } else if !printed_any_text {
                        // Daemon sent no incremental deltas; emit the final reply.
                        if let Some(reply) = event.get("reply").and_then(|v| v.as_str()) {
                            println!("{reply}");
                            let _ = std::io::stdout().flush();
                        }
                    }
                }
                "error" => {
                    let message = event
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown daemon error");
                    attach_fail(message.to_string(), is_json);
                }
                _ => {
                    if is_json {
                        println!("{}", serde_json::to_string(&event).unwrap_or_default());
                        let _ = std::io::stdout().flush();
                    }
                }
            }
        }
    }

    if !saw_done {
        attach_fail(
            "daemon stream ended without a completion event".to_string(),
            is_json,
        );
    }

    // 4. Best-effort detach so the daemon releases this process's lease
    //    promptly instead of waiting for the staleness timeout. Errors are
    //    ignored -- the turn already completed successfully.
    let detach_request = SessionDetachRequest {
        session_id: session_id.clone(),
        client_id: Some(client_id),
    };
    let detach_url = format!("{base_url}/api/v1/runtimes/detach");
    let mut detach_req = client.post(&detach_url).json(&detach_request);
    if let Some(token) = bearer_token.as_ref() {
        detach_req = detach_req.bearer_auth(token);
    }
    let _ = detach_req.send().await;
}

/// Report an attach-mode failure to stderr (text) or stdout (JSON), then exit.
fn attach_fail(message: String, is_json: bool) -> ! {
    if is_json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "type": "error",
                "data": { "message": message }
            }))
            .unwrap()
        );
        let _ = std::io::stdout().flush();
    } else {
        eprintln!("{message}");
    }
    std::process::exit(1);
}

/// Extract a single SSE frame (text up to a blank line) from the buffer.
fn take_sse_frame(buffer: &mut String) -> Option<String> {
    let index = buffer.find("\n\n")?;
    let frame = buffer[..index].to_string();
    buffer.drain(..index + 2);
    Some(frame)
}

/// Parse an SSE frame's `data:` lines into a single JSON value. Returns
/// `None` for keep-alive/comment frames or malformed JSON.
fn parse_sse_event(frame: &str) -> Option<Value> {
    let mut data_lines = Vec::new();
    for line in frame.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') || line.is_empty() {
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start());
        }
    }
    if data_lines.is_empty() {
        return None;
    }
    let data = data_lines.join("\n");
    serde_json::from_str(&data).ok()
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
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parses_explicit_mcp_config_path() {
        let args = Args::try_parse_from([
            "xiaoo",
            "--mcp-config",
            "/tmp/mcp.json",
            "run",
            "--prompt",
            "hello",
        ])
        .expect("CLI should accept --mcp-config");

        assert_eq!(
            args.mcp_config.as_deref(),
            Some(std::path::Path::new("/tmp/mcp.json"))
        );
    }

    #[test]
    fn copy_dir_rejects_destination_inside_source() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("skills");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("SKILL.md"), "test").unwrap();

        let err = copy_dir_recursive(&src, &src.join("nested")).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_generate_title_from_prompt() {
        let prompt = "Fix the bug in authentication module related to JWT token validation";
        let title = generate_title_from_prompt(prompt);
        assert_eq!(
            title,
            Some("Fix the bug in authentication module related to JWT token".to_string())
        );
    }

    #[test]
    fn test_generate_title_from_short_prompt() {
        let prompt = "Hello world";
        let title = generate_title_from_prompt(prompt);
        assert_eq!(title, Some("Hello world".to_string()));
    }

    #[test]
    fn test_generate_title_from_empty_prompt() {
        let prompt = "";
        let title = generate_title_from_prompt(prompt);
        assert_eq!(title, None);
    }

    #[test]
    fn test_generate_title_from_whitespace_prompt() {
        let prompt = "   ";
        let title = generate_title_from_prompt(prompt);
        assert_eq!(title, None);
    }
}

#[cfg(test)]
mod attach_sse_tests {
    use super::{parse_sse_event, take_sse_frame};

    #[test]
    fn take_sse_frame_extracts_frame_and_drains() {
        let mut buf = String::from("data: {\"type\":\"x\"}\n\nleftover");
        let frame = take_sse_frame(&mut buf).unwrap();
        assert_eq!(frame, "data: {\"type\":\"x\"}");
        assert_eq!(buf, "leftover");
    }

    #[test]
    fn take_sse_frame_returns_none_without_blank_line() {
        let mut buf = String::from("data: partial");
        assert!(take_sse_frame(&mut buf).is_none());
        assert_eq!(buf, "data: partial");
    }

    #[test]
    fn parse_sse_event_parses_data_json() {
        let frame = "data: {\"type\":\"text_delta\",\"delta\":\"hi\"}";
        let event = parse_sse_event(frame).unwrap();
        assert_eq!(event["type"], "text_delta");
        assert_eq!(event["delta"], "hi");
    }

    #[test]
    fn parse_sse_event_joins_multi_line_data() {
        let frame = "data: {\"type\":\"done\",\ndata: \"reply\":\"ok\"}";
        let event = parse_sse_event(frame).unwrap();
        assert_eq!(event["type"], "done");
        assert_eq!(event["reply"], "ok");
    }

    #[test]
    fn parse_sse_event_ignores_comments_and_blank_lines() {
        assert!(parse_sse_event(": keepalive\n\n").is_none());
    }

    #[test]
    fn parse_sse_event_returns_none_for_malformed_json() {
        assert!(parse_sse_event("data: {not json").is_none());
    }
}
