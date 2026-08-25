use anyhow::{bail, Context, Result};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use operation_backend::process_group::ProcessGroupCleanupGuard;
use std::env;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;

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
    let config = load_tui_config(&config_arg)?;
    config::inject_llm_secrets_into_env(&config_arg.path).with_context(|| {
        format!(
            "failed to initialize TUI secrets from {}",
            config_arg.path.display()
        )
    })?;
    let mut config = config::require_tui_bootstrap_config(config, &config_arg.path)?;
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    config.load_runtime_mcp_servers(
        config_arg.mcp_config.as_deref(),
        &workspace,
        dirs::home_dir().as_deref(),
        &config_arg.path,
    )?;
    run_tui(config, config_arg.path, workspace).await
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

fn load_tui_config(config_arg: &ConfigArg) -> Result<Option<config::Config>> {
    if !config_arg.path.exists() {
        if config_arg.explicit {
            bail!("config file not found: {}", config_arg.path.display());
        }
        return Ok(None);
    }
    config::Config::load_from(&config_arg.path).map(Some)
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
        return dirs::home_dir()
            .map(|home| home.join(".config").join("xiaoo").join("config.toml"))
            .ok_or_else(|| anyhow::anyhow!("unable to resolve ~/.config/xiaoo/config.toml"));
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

async fn run_tui(config: config::Config, config_path: PathBuf, workspace: PathBuf) -> Result<()> {
    let (validation_errors, validation_warnings) = validate_config_for_tui(&config, &config_path);

    for warning in &validation_warnings {
        tracing::warn!("Config validation warning: {}", warning);
    }

    if !validation_errors.is_empty() {
        for error in &validation_errors {
            eprintln!("! Config Error: {}", error);
            eprintln!("Program startup failed due to invalid configuration.");
            eprintln!("Please fix the configuration in: {}", config_path.display());
            std::process::exit(1);
        }
    }

    enable_raw_mode().context("failed to enable terminal raw mode")?;
    execute!(io::stdout(), EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
    let mut terminal =
        ratatui::Terminal::new(backend).context("failed to create TUI terminal backend")?;
    let _ = execute!(io::stdout(), SetCursorStyle::BlinkingBar);
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let _ = execute!(io::stdout(), EnableBracketedPaste);

    let mut app = app::App::new_with_config(&config, config_path.clone(), workspace)
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
            app.gateway.configure_remote(
                &mut app.state,
                remote.url.clone(),
                remote.bearer_token_env.clone(),
            );
            app.state
                .chat_state
                .messages
                .push(crate::chat::Message::system(format!(
                    "Remote backend configured: {}",
                    remote.url.trim()
                )));
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

/// Validate configuration for TUI
fn validate_config_for_tui(
    config: &config::Config,
    config_path: &PathBuf,
) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let resolved_context_window = config::resolve_context_window(config).unwrap_or(128_000);
    let max_tokens = config.llm.max_tokens;

    let max_reasonable_output_ratio = 0.5;
    let max_reasonable_output_tokens =
        (resolved_context_window as f64 * max_reasonable_output_ratio) as usize;

    if max_tokens as usize > max_reasonable_output_tokens {
        warnings.push(format!(
            "max_tokens {} exceeds 50% of the estimated context window for model {}. \
            This may limit input space.",
            max_tokens, config.llm.model
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
mod tests {
    use super::{classify_args, parse_config_path_from, EntryInvocation};
    use std::ffi::OsString;

    #[test]
    fn tui_accepts_mcp_config_alongside_toml_config() {
        let parsed = parse_config_path_from([
            "xiaoo",
            "--config",
            "/tmp/config.toml",
            "--mcp-config",
            "/tmp/mcp.json",
        ])
        .expect("TUI should accept both config paths");

        assert_eq!(parsed.path, std::path::PathBuf::from("/tmp/config.toml"));
        assert_eq!(
            parsed.mcp_config,
            Some(std::path::PathBuf::from("/tmp/mcp.json"))
        );
    }

    #[test]
    fn no_args_dispatches_to_tui() {
        assert_eq!(
            classify_args(vec![OsString::from("xiaoo")]),
            EntryInvocation::Tui(vec![OsString::from("xiaoo")])
        );
    }

    #[test]
    fn cli_switch_dispatches_to_cli_without_switch() {
        assert_eq!(
            classify_args(vec![
                OsString::from("xiaoo"),
                OsString::from("--cli"),
                OsString::from("run"),
                OsString::from("-p"),
                OsString::from("hello"),
            ]),
            EntryInvocation::Cli(vec![
                OsString::from("xiaoo"),
                OsString::from("run"),
                OsString::from("-p"),
                OsString::from("hello"),
            ])
        );
    }

    #[test]
    fn help_dispatches_to_end_side_help() {
        assert_eq!(
            classify_args(vec![OsString::from("xiaoo"), OsString::from("--help")]),
            EntryInvocation::Help {
                program: OsString::from("xiaoo"),
            }
        );
    }
}
