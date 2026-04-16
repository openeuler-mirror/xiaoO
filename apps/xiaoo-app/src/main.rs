mod daemon_config;
mod daemon_runtime;

use crate::daemon_config::{resolve_config_path, DaemonConfig};
use crate::daemon_runtime::ConfiguredRuntimeResolver;
use anyhow::{bail, Context, Result};
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use xiaoo_app::gateway::{AppBootstrap, InMemorySessionStore, SessionStore};
use xiaoo_app::httpserver::{create_router, create_router_with_feishu_and_timeout};

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
    let resolver = Arc::new(ConfiguredRuntimeResolver::from_config(&config)?);
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let app = AppBootstrap::from_session_components(session_store, resolver)?;
    let router = match config.feishu_config()? {
        Some(feishu) => create_router_with_feishu_and_timeout(
            app.session_service,
            feishu,
            config.interaction_timeout_secs(),
        )
        .map_err(anyhow::Error::new)
        .context("failed to create router with feishu")?,
        None => create_router(app.session_service),
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
                let mut port = 8080_u16;
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
}
