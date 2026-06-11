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
pub(crate) use services::provider as provider_service;
pub(crate) use services::session_snapshot as session_snapshot_service;
pub(crate) use services::skills as skills_service;
pub(crate) use services::workspace as workspace_service;
pub(crate) use state::app_state;
pub(crate) use state::chat;
pub(crate) use state::selection;
pub(crate) use support::config;
pub(crate) use support::debug_log;
pub(crate) use support::error_log;

const CONFIG_ENV_VAR: &str = "XIAOO_CONFIG";
const CLI_SWITCH: &str = "--cli";

#[tokio::main]
async fn main() -> Result<()> {
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
    config::load_llm_secrets_to_memory(&config_arg.path).with_context(|| {
        format!(
            "failed to initialize TUI secrets from {}",
            config_arg.path.display()
        )
    })?;
    let config = config::require_tui_bootstrap_config(config, &config_arg.path)?;
    run_tui(config, config_arg.path).await
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
        "Usage: {} [--config <path>]\n       {} --cli <command>\n\nDefault: launch the TUI.\nCLI: pass --cli before existing CLI commands, for example `{} --cli run -p \"hello\"`.",
        PathBuf::from(program).display(),
        PathBuf::from(program).display(),
        PathBuf::from(program).display()
    );
}

struct ConfigArg {
    path: PathBuf,
    explicit: bool,
}

fn parse_config_path_from<I, T>(args: I) -> Result<ConfigArg>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut args = args.into_iter().map(Into::into);
    let program = args.next().unwrap_or_else(|| OsString::from("xiaoo"));

    let cli_path = match args.next() {
        None => None,
        Some(first) if first == "--help" || first == "-h" => {
            print_usage(&program);
            std::process::exit(0);
        }
        Some(first) if first == "--config" || first == "-c" => {
            let Some(path) = args.next() else {
                bail!("missing value for --config");
            };
            if args.next().is_some() {
                bail!("unexpected extra arguments after --config");
            }
            Some(PathBuf::from(path))
        }
        Some(_) => {
            bail!("unsupported arguments. use --help for usage, or pass only --config <path>")
        }
    };

    if let Some(path) = cli_path {
        return Ok(ConfigArg {
            path,
            explicit: true,
        });
    }

    if let Some(path) = std::env::var_os(CONFIG_ENV_VAR)
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Ok(ConfigArg {
            path,
            explicit: true,
        });
    }

    Ok(ConfigArg {
        path: default_config_path()?,
        explicit: false,
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
        "Usage: {} [--config <path>]\n\nConfig lookup order: --config > XIAOO_CONFIG > platform default.\nLaunch the TUI binary directly.",
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

async fn run_tui(config: config::Config, config_path: PathBuf) -> Result<()> {
    populate_effective_context_window(&config).await;

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

    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
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

async fn populate_effective_context_window(config: &config::Config) {
    tracing::debug!("Starting context window detection process");

    let configured_max_tokens = config.llm.max_tokens as usize;
    let conservative_limit = 400_000;

    if configured_max_tokens > conservative_limit {
        tracing::warn!(
            configured_max_tokens,
            conservative_limit,
            provider = &config.llm.provider,
            model = &config.llm.model,
            "⚠ max_tokens {} exceeds conservative limit {}. \
             High risk of API rejection. Consider reducing max_tokens.",
            configured_max_tokens,
            conservative_limit
        );
    }

    tracing::info!("Querying model catalog API for context window");

    let resolved = llm_client::resolve_config(llm_client::ResolveInput {
        provider: Some(config.llm.provider.clone()),
        protocol: None,
        api_key: None,
        api_key_env: config.llm.api_key_env.clone(),
        base_url: if config.llm.api_base.trim().is_empty() {
            None
        } else {
            Some(config.llm.api_base.clone())
        },
    });

    let resolved_ok: Option<llm_client::ResolvedConfig> = match &resolved {
        Ok(r) => Some(r.clone()),
        Err(e) => {
            tracing::warn!(
                source = "catalog_resolution_failed",
                error = e.to_string(),
                "Failed to resolve provider config for catalog query"
            );
            None
        }
    };

    if let Some(resolved) = &resolved_ok {
        match llm_client::resolve_model_context_length(resolved, &config.llm.model).await {
            Ok(Some(context_window)) => {
                tracing::info!(
                    source = "model_catalog",
                    context_window,
                    provider = &config.llm.provider,
                    model = &config.llm.model,
                    "✓ Context window detected from model catalog: {}",
                    context_window
                );
                return;
            }
            Ok(None) => {
                tracing::info!(
                    source = "catalog_not_found",
                    provider = &config.llm.provider,
                    model = &config.llm.model,
                    "Catalog did not return context_window for this model"
                );
            }
            Err(error) => {
                tracing::warn!(
                    source = "catalog_query_failed",
                    provider = &config.llm.provider,
                    model = &config.llm.model,
                    error = error.to_string(),
                    "Catalog query failed"
                );
            }
        }
    }

    if let Some(context_window) = config::resolve_context_window(config) {
        tracing::info!(
            source = "protocol_default",
            context_window,
            provider = &config.llm.provider,
            model = &config.llm.model,
            "Using protocol default context_window (runtime will auto-adjust if needed)"
        );
    } else {
        tracing::warn!(
            provider = &config.llm.provider,
            model = &config.llm.model,
            "Could not determine context_window, runtime will auto-detect from API errors"
        );
    }
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
            "max_tokens {} exceeds 50% of context_window ({} * 0.5 = {}) for model {}. \
            This may limit input space.",
            max_tokens, resolved_context_window, max_reasonable_output_tokens, config.llm.model
        ));
    }

    if max_tokens as usize >= resolved_context_window {
        errors.push(format!(
            "max_tokens {} >= context_window {} (auto-detected). \
            This would leave NO space for input.\n\
            Configuration file: {}\n\
            Suggestions:\n\
              - Reduce max_tokens (currently {}) in [llm] section",
            max_tokens,
            resolved_context_window,
            config_path.display(),
            max_tokens,
        ));
    }

    (errors, warnings)
}

#[cfg(test)]
mod tests {
    use super::{classify_args, EntryInvocation};
    use std::ffi::OsString;

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
