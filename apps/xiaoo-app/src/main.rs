mod daemon_config;
mod daemon_runtime;
mod lsp_support;

use crate::daemon_config::{resolve_config_path, DaemonConfig};
use crate::daemon_runtime::ConfiguredRuntimeResolver;
use anyhow::{bail, Context, Result};
use futures_util::future::BoxFuture;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use xiaoo_app::channels::{
    build_telegram_runtime, TelegramPollingMessageHandler, TelegramPollingService,
};
use xiaoo_app::gateway::{AppBootstrap, InMemorySessionStore, SessionStore};
use xiaoo_app::httpserver::{
    create_router_with_channel_runtimes_control_plane_and_timeout_and_auth,
    create_router_with_control_plane_and_auth, ChannelRuntimeProcessor, HttpBearerAuthConfig,
};

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse(env::args().skip(1))?;
    match cli.command {
        Command::Daemon { config, host, port } => run_daemon(config, host, port).await,
    }
}

async fn run_daemon(config_path: Option<PathBuf>, host: String, port: u16) -> Result<()> {
    let config_path = resolve_config_path(config_path)?;
    let config = DaemonConfig::load_from(&config_path)?;
    let hooker_config = config.app.hooker.clone();
    let bearer_auth = config.http_bearer_token()?.map(HttpBearerAuthConfig::new);
    let rate_limit = config.app.http.rate_limit.clone();
    let resolver = Arc::new(ConfiguredRuntimeResolver::from_config(&config).await?);
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let app =
        AppBootstrap::from_session_components_with_hooks(session_store, resolver, hooker_config)?;
    let interaction_timeout_secs = config.interaction_timeout_secs();
    if let Some(telegram_config) = config.telegram_polling_config()? {
        spawn_telegram_polling_service(
            telegram_config,
            app.session_service.clone(),
            interaction_timeout_secs,
        )
        .context("failed to start telegram polling service")?;
    }
    let channel_runtimes = config.channel_runtimes()?;
    let router = if channel_runtimes.is_empty() {
        create_router_with_control_plane_and_auth(
            app.session_service,
            app.session_control_plane,
            bearer_auth,
            rate_limit,
        )
    } else {
        create_router_with_channel_runtimes_control_plane_and_timeout_and_auth(
            app.session_service,
            app.session_control_plane,
            channel_runtimes,
            interaction_timeout_secs,
            bearer_auth,
            rate_limit,
        )
        .map_err(anyhow::Error::new)
        .context("failed to create router with channel runtimes")?
    };

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid listen address {host}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(config = %config_path.display(), %addr, "starting rebuild daemon");
    axum::serve(listener, router)
        .await
        .context("axum server exited unexpectedly")
}

fn spawn_telegram_polling_service(
    telegram_config: xiaoo_app::channels::TelegramConfig,
    session_service: Arc<dyn xiaoo_app::gateway::SessionService>,
    interaction_timeout_secs: u64,
) -> Result<()> {
    let runtime = build_telegram_runtime(telegram_config.clone()).map_err(anyhow::Error::new)?;
    let processor =
        ChannelRuntimeProcessor::with_timeout(session_service, interaction_timeout_secs);
    let service = TelegramPollingService::new(telegram_config).map_err(anyhow::Error::new)?;
    let handler: TelegramPollingMessageHandler = Arc::new(move |message| {
        let processor = processor.clone();
        let runtime = runtime.clone();
        Box::pin(async move {
            if let Err(error) = processor.process_message(runtime, message).await {
                tracing::warn!("failed to process telegram polling message: {error}");
            }
        }) as BoxFuture<'static, ()>
    });
    tracing::info!("starting telegram polling transport");
    tokio::spawn(async move {
        service.run_forever(handler).await;
    });
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,xiaoo_app=debug"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

struct Cli {
    command: Command,
}

enum Command {
    Daemon {
        config: Option<PathBuf>,
        host: String,
        port: u16,
    },
}

impl Cli {
    fn parse<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut args = args.into_iter();
        let Some(command) = args.next() else {
            bail!("missing command: expected `daemon`");
        };
        match command.as_str() {
            "daemon" => {
                let mut config = None;
                let mut host = "0.0.0.0".to_string();
                let mut port = 18080_u16;
                let remaining = args.collect::<Vec<_>>();
                let mut index = 0;
                while index < remaining.len() {
                    match remaining[index].as_str() {
                        "--config" => {
                            index += 1;
                            let value =
                                remaining.get(index).context("missing value for --config")?;
                            config = Some(PathBuf::from(value));
                        }
                        "--host" => {
                            index += 1;
                            let value = remaining.get(index).context("missing value for --host")?;
                            host = value.clone();
                        }
                        "--port" => {
                            index += 1;
                            let value = remaining.get(index).context("missing value for --port")?;
                            port = value
                                .parse()
                                .with_context(|| format!("invalid port `{value}`"))?;
                        }
                        other => bail!("unknown argument `{other}` for daemon"),
                    }
                    index += 1;
                }
                Ok(Self {
                    command: Command::Daemon { config, host, port },
                })
            }
            other => bail!("unknown command `{other}`"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;

    #[test]
    fn parses_daemon_arguments() {
        let cli = Cli::parse(
            [
                "daemon",
                "--config",
                "/tmp/demo.toml",
                "--host",
                "127.0.0.1",
                "--port",
                "18080",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("cli should parse");

        assert!(matches!(cli.command, super::Command::Daemon { .. }));
    }

    #[test]
    fn daemon_defaults_to_port_18080() {
        let cli = Cli::parse(["daemon"].into_iter().map(str::to_string))
            .expect("cli should parse with defaults");

        match cli.command {
            super::Command::Daemon { host, port, .. } => {
                assert_eq!(host, "0.0.0.0");
                assert_eq!(port, 18080);
            }
        }
    }
}
