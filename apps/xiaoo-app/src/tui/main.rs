use anyhow::{bail, Context, Result};
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use std::io;
use std::path::PathBuf;

pub use xiaoo_app::gateway;

mod app;
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

const CONFIG_ENV_VAR: &str = "XIAOO_CONFIG";

#[tokio::main]
async fn main() -> Result<()> {
    let config_arg = parse_config_path()?;
    let config = load_tui_config(&config_arg)?;
    config::inject_llm_secrets_into_env(&config_arg.path).with_context(|| {
        format!(
            "failed to initialize TUI secrets from {}",
            config_arg.path.display()
        )
    })?;
    let config = config::require_tui_bootstrap_config(config, &config_arg.path)?;
    run_tui(config, config_arg.path).await
}

struct ConfigArg {
    path: PathBuf,
    explicit: bool,
}

fn parse_config_path() -> Result<ConfigArg> {
    let mut args = std::env::args_os();
    let program = args
        .next()
        .unwrap_or_else(|| std::ffi::OsString::from("xiaoo-app"));

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

async fn run_tui(mut config: config::Config, config_path: PathBuf) -> Result<()> {
    populate_effective_context_window(&mut config).await;

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

async fn populate_effective_context_window(config: &mut config::Config) {
    if config
        .llm
        .context_window
        .filter(|value| *value > 0)
        .is_some()
    {
        return;
    }

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

    match resolved {
        Ok(resolved) => {
            match llm_client::resolve_model_context_length(&resolved, &config.llm.model).await {
                Ok(Some(context_window)) => match u32::try_from(context_window) {
                    Ok(value) => {
                        config.llm.context_window = Some(value);
                        return;
                    }
                    Err(_) => {
                        tracing::warn!(
                            provider = %config.llm.provider,
                            model = %config.llm.model,
                            context_window,
                            "dynamic context window does not fit into TUI config type; falling back"
                        );
                    }
                },
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        provider = %config.llm.provider,
                        model = %config.llm.model,
                        error = %error,
                        "failed to dynamically resolve model context window; falling back"
                    );
                }
            }
        }
        Err(error) => {
            tracing::warn!(
                provider = %config.llm.provider,
                model = %config.llm.model,
                error = %error,
                "failed to resolve provider config for dynamic context window lookup; falling back"
            );
        }
    }

    if let Some(context_window) =
        config::resolve_context_window(config).and_then(|value| u32::try_from(value).ok())
    {
        config.llm.context_window = Some(context_window);
    }
}
