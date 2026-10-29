use anyhow::{bail, Context, Result};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use xiaoo_shared::backend::ProcessGroupCleanupGuard;
use xiaoo_shared::config_identity::ConfigFileIdentity;
pub use xiaoo_shared::{backend, gateway};

mod app;
mod cli;
mod gateway_api;
mod input;
mod render;
mod services;
mod state;
mod support;

pub(crate) use gateway_api::runtime as gateway_runtime;
pub(crate) use gateway_api::session as session_gateway;
pub(crate) use input::slash_complete;
pub(crate) use render::interaction_prompt;
pub(crate) use render::markdown;
pub(crate) use render::provider_dialog;
pub(crate) use render::status_panel;
pub(crate) use render::theme;
pub(crate) use services::mcp as mcp_service;
pub(crate) use services::provider as provider_service;
pub(crate) use services::remote_sessions as remote_sessions_service;
pub(crate) use services::session_snapshot as session_snapshot_service;
pub(crate) use services::skills as skills_service;
pub(crate) use services::workspace as workspace_service;
pub(crate) use state::app_state;
pub(crate) use state::chat;
pub(crate) use state::cron_dialog;
pub(crate) use state::selection;
pub(crate) use support::config;
pub(crate) use support::debug_log;
pub(crate) use support::error_log;

const CONFIG_ENV_VAR: &str = "XIAOO_CONFIG";
const CLI_SWITCH: &str = "--cli";

/// Total budget startup validation waits for dynamic context-window probing:
/// the probe chain's per-client timeouts add up to ~40s, and startup cannot
/// wait unbounded. Past this deadline we fall back to the protocol-family
/// static value and continue.
const CONFIG_PROBE_DEADLINE: Duration = Duration::from_secs(3);

#[tokio::main]
async fn main() -> Result<()> {
    let _cleanup_guard = ProcessGroupCleanupGuard;

    match classify_args(env::args_os().collect()) {
        EntryInvocation::Help { program } => {
            print_end_side_usage(&program);
            Ok(())
        }
        EntryInvocation::Cli(args) => {
            cli::entry::run_cli_from_args(args).await;
            Ok(())
        }
        EntryInvocation::Tui(args) => run_tui_from_args(args).await,
    }
}

pub async fn run_tui_from_args<I, T>(args: I) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let config_arg = parse_config_path_from(args)?;
    let loaded = load_tui_config(&config_arg)?;
    config::init_llm_secrets(&config_arg.path)?;
    // Identity pairs with the config from the same read (see
    // load_tui_config).
    let (config, config_identity) = loaded.unzip();
    let mut config = config::require_tui_bootstrap_config(config, &config_arg.path)?;
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    config.load_runtime_mcp_servers(
        config_arg.mcp_config.as_deref(),
        &workspace,
        dirs::home_dir().as_deref(),
        &config_arg.path,
    )?;
    run_tui(config, config_arg.path, workspace, config_identity).await
}

#[derive(Debug, PartialEq, Eq)]
enum EntryInvocation {
    Help { program: OsString },
    Cli(Vec<OsString>),
    Tui(Vec<OsString>),
}

fn classify_args(mut args: Vec<OsString>) -> EntryInvocation {
    let program = args
        .first()
        .cloned()
        .unwrap_or_else(|| OsString::from("xiaoo"));
    match args.get(1) {
        Some(first) if os_str_eq(first, "--help") || os_str_eq(first, "-h") => {
            EntryInvocation::Help { program }
        }
        Some(first) if os_str_eq(first, CLI_SWITCH) => {
            args.remove(1);
            EntryInvocation::Cli(args)
        }
        _ => EntryInvocation::Tui(args),
    }
}

fn os_str_eq(value: &OsStr, expected: &str) -> bool {
    value == OsStr::new(expected)
}

fn print_end_side_usage(program: &OsStr) {
    eprintln!(
        "Usage: {} [--config <path>] [--mcp-config <path>]\n       {} --cli <command>\n\nDefault: launch the TUI.\nCLI: pass --cli before existing CLI commands, for example `{} --cli run -p \"hello\"`.",
        PathBuf::from(program).display(),
        PathBuf::from(program).display(),
        PathBuf::from(program).display()
    );
}

struct ConfigArg {
    path: PathBuf,
    explicit: bool,
    mcp_config: Option<PathBuf>,
}

fn parse_config_path_from<I, T>(args: I) -> Result<ConfigArg>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let program = args.next().unwrap_or_else(|| OsString::from("xiaoo"));

    let mut cli_path = None;
    let mut mcp_config = None;
    while let Some(argument) = args.next() {
        if argument == "--help" || argument == "-h" {
            print_usage(&program);
            std::process::exit(0);
        } else if argument == "--config" || argument == "-c" {
            let Some(path) = args.next() else {
                bail!("missing value for --config");
            };
            if cli_path.replace(PathBuf::from(path)).is_some() {
                bail!("--config may only be specified once");
            }
        } else if argument == "--mcp-config" {
            let Some(path) = args.next() else {
                bail!("missing value for --mcp-config");
            };
            if mcp_config.replace(PathBuf::from(path)).is_some() {
                bail!("--mcp-config may only be specified once");
            }
        } else {
            bail!("unsupported argument {:?}. use --help for usage", argument);
        }
    }

    if let Some(path) = cli_path {
        return Ok(ConfigArg {
            path,
            explicit: true,
            mcp_config,
        });
    }

    if let Some(path) = std::env::var_os(CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Ok(ConfigArg {
            path,
            explicit: true,
            mcp_config,
        });
    }

    Ok(ConfigArg {
        path: default_config_path()?,
        explicit: false,
        mcp_config,
    })
}

/// Load the config and its identity snapshot from a single read: the
/// snapshot must hash the bytes that actually entered memory.
fn load_tui_config(config_arg: &ConfigArg) -> Result<Option<(config::Config, ConfigFileIdentity)>> {
    if !config_arg.path.exists() {
        if config_arg.explicit {
            bail!("config file not found: {}", config_arg.path.display());
        }
        return Ok(None);
    }
    let (config, content) = config::Config::load_from_with_content(&config_arg.path)?;
    let identity = ConfigFileIdentity::from_loaded_content(&config_arg.path, content.as_bytes());
    Ok(Some((config, identity)))
}

fn print_usage(program: &std::ffi::OsStr) {
    eprintln!(
        "Usage: {} [--config <path>] [--mcp-config <path>]\n\nConfig lookup order: --config > XIAOO_CONFIG > platform default.\nMCP lookup order: --mcp-config > XIAOO_MCP_CONFIG > workspace .mcp.json > ~/.config/xiaoo/mcp.json.\nLaunch the TUI binary directly.",
        PathBuf::from(program).display()
    );
}

fn default_config_path() -> Result<PathBuf> {
    #[cfg(unix)]
    {
        dirs::home_dir()
            .map(|home| home.join(".config").join("xiaoo").join("config.toml"))
            .ok_or_else(|| anyhow::anyhow!("unable to resolve ~/.config/xiaoo/config.toml"))
    }

    #[cfg(windows)]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|dir| dir.join("xiaoo").join("config.toml"))
            .ok_or_else(|| anyhow::anyhow!("unable to resolve %APPDATA%\\xiaoo\\config.toml"));
    }

    #[cfg(not(any(unix, windows)))]
    {
        return dirs::config_dir()
            .map(|dir| dir.join("xiaoo").join("config.toml"))
            .ok_or_else(|| anyhow::anyhow!("unable to resolve platform config path"));
    }
}

async fn run_tui(
    config: config::Config,
    config_path: PathBuf,
    workspace: PathBuf,
    config_identity: Option<ConfigFileIdentity>,
) -> Result<()> {
    let (validation_errors, validation_warnings) =
        validate_config_for_tui(&config, &config_path).await;

    for warning in &validation_warnings {
        tracing::warn!("Config validation warning: {}", warning);
    }

    if !validation_errors.is_empty() {
        for error in &validation_errors {
            eprintln!("! Config Error: {}", error);
        }
        eprintln!("Program startup failed due to invalid configuration.");
        eprintln!("Please fix the configuration in: {}", config_path.display());
        std::process::exit(1);
    }

    enable_raw_mode().context("failed to enable terminal raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen).context("failed to enter alternate screen")?;
    // FrameBackend stages each frame and hands it to the tty in a single
    // write — see app/frame_backend.rs for why the stock CrosstermBackend
    // over stdout splits frames and makes the caret flicker.
    let mut terminal = ratatui::Terminal::new(app::frame_backend::stdout_frame_backend())
        .context("failed to create TUI terminal backend")?;
    let _ = execute!(io::stdout(), SetCursorStyle::SteadyBar);
    let _ = execute!(io::stdout(), EnableMouseCapture);
    // `EnableMouseCapture` enables all-motion tracking (`?1003h`), which
    // makes the terminal report every mouse-move as an SGR byte sequence
    // (`\x1b[<b;x;yM`). xiaoO never uses plain `Moved` events (hover-less
    // UI; scrollbar dragging uses `Drag` from `?1002h`), but the byte storm
    // interleaves with keyboard input: when an ESC key lands next to a mouse
    // sequence, crossterm's parser swallows the sequence head and the
    // leftover `[<b;x;yM` is delivered as plain characters, which then get
    // typed into the input box. Disable all-motion tracking, then explicitly
    // select button-event tracking (`?1002h`) for clicks, drags and the wheel.
    // iTerm2 treats 1000/1002/1003 as mutually exclusive modes: `?1003l`
    // disables mouse reporting entirely instead of falling back to 1002.
    // SGR (`?1006h`) remains enabled by `EnableMouseCapture`.
    let _ = io::stdout().write_all(b"\x1b[?1003l\x1b[?1002h");
    let _ = io::stdout().flush();
    let _ = execute!(io::stdout(), EnableBracketedPaste);

    let mut app =
        app::App::new_with_config(&config, config_path.clone(), workspace, config_identity)
            .context("failed to initialize TUI app state")?;

    for error in validation_errors {
        app.state
            .chat_state
            .messages
            .push(crate::chat::Message::system(format!(
                "! Config Error: {}",
                error
            )));
    }

    for warning in validation_warnings {
        app.state
            .chat_state
            .messages
            .push(crate::chat::Message::system(format!(
                "! Config Warning: {}",
                warning
            )));
    }

    if let Some(remote) = config
        .tui
        .remote
        .as_ref()
        .filter(|remote| remote.auto_connect)
    {
        if !remote.url.trim().is_empty() {
            // auto_connect goes through connect_remote like a manual
            // `/remote <url>`: configure_remote alone would skip the
            // same-config check.
            let message = match app
                .gateway
                .connect_remote(
                    &mut app.state,
                    remote.url.clone(),
                    remote.bearer_token_env.clone(),
                )
                .await
            {
                Ok(message) => crate::chat::Message::system(message),
                Err(error) => {
                    crate::chat::Message::error(format!("Remote connect failed: {error}"))
                }
            };
            app.state.chat_state.messages.push(message);
        }
    }

    let result = app.run(&mut terminal).await;

    let _ = execute!(io::stdout(), DisableBracketedPaste);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = execute!(io::stdout(), SetCursorStyle::DefaultUserShape);
    let _ = disable_raw_mode();

    result
}

/// Validate configuration for TUI.
///
/// Prefer the dynamic probing chain for the context window
/// ([`crate::gateway_api::runtime_request::probe_dynamic_context_window`],
/// the same one the TUI gateway uses), capped by an inline deadline; on
/// timeout/failure fall back to the protocol-family static value
/// ([`config::resolve_context_window`]). That way a large-`max_tokens`
/// config on a large-window model is not rejected as an error just because
/// the 128k/200k static fallback underestimates it.
async fn validate_config_for_tui(
    config: &config::Config,
    config_path: &Path,
) -> (Vec<String>, Vec<String>) {
    let resolved_context_window = resolve_context_window_for_validation(config).await;
    validate_max_tokens_against_window(
        config.llm.max_tokens,
        &config.llm.model,
        resolved_context_window,
        config_path,
    )
}

/// Window resolution for startup validation: goes through the same
/// process-wide cache and probe chain as the TUI gateway
/// (`resolve_window_with_shared_cache`); the probe writes the cache, so the
/// first turn hits it and skips a repeat wait.
async fn resolve_context_window_for_validation(config: &config::Config) -> usize {
    crate::gateway_api::runtime_request::resolve_window_with_shared_cache(
        &config.llm,
        config::resolve_context_window(config),
        CONFIG_PROBE_DEADLINE,
    )
    .await
    .unwrap_or(128_000)
}

/// Pure function: resolved window + config in, `(errors, warnings)` out.
/// Kept standalone so unit tests can inject a synthetic window without
/// touching the network.
fn validate_max_tokens_against_window(
    max_tokens: u32,
    model: &str,
    resolved_context_window: usize,
    config_path: &Path,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let max_reasonable_output_ratio = 0.5;
    let max_reasonable_output_tokens =
        (resolved_context_window as f64 * max_reasonable_output_ratio) as usize;

    if max_tokens as usize > max_reasonable_output_tokens {
        warnings.push(format!(
            "max_tokens {} exceeds 50% of the estimated context window for model {}. \
            This may limit input space.",
            max_tokens, model
        ));
    }

    if max_tokens as usize >= resolved_context_window {
        errors.push(format!(
            "max_tokens {} is too large relative to the estimated context window. \
            This would leave NO space for input.\n\
            Configuration file: {}\n\
            Suggestions:\n\
              - Reduce max_tokens (currently {}) in [llm] section",
            max_tokens,
            config_path.display(),
            max_tokens,
        ));
    }

    (errors, warnings)
}

#[cfg(test)]
#[path = "../../../tests/unit/endside/main_test.rs"]
mod tests;
