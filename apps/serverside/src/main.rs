mod channels;
mod cron;
mod daemon_config;
mod daemon_runtime;
mod httpserver;
mod mcp_server;

use crate::channels::{
    build_feishu_runtime, build_telegram_runtime, FeishuConfig, FeishuEventTransport,
    FeishuWebsocketMessageHandler, FeishuWebsocketService, TelegramConfig,
    TelegramPollingMessageHandler, TelegramPollingService,
};
use crate::cron::scheduler::CronScheduler;
use crate::daemon_config::{resolve_config_path, DaemonConfig};
use crate::daemon_runtime::ConfiguredRuntimeResolver;
use crate::httpserver::{
    create_router_with_channel_runtimes_control_plane_and_timeout_and_auth,
    create_router_with_control_plane_and_auth, dashboard_router, ChannelRuntimeProcessor,
    DashboardState, HttpBearerAuthConfig,
};
use crate::mcp_server::create_mcp_router;
use anyhow::{bail, Context, Result};
use futures_util::future::BoxFuture;
use operation_backend::process_group::ProcessGroupCleanupGuard;
use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use xiaoo_shared::backend::BackendManager;
use xiaoo_shared::gateway::{AppBootstrap, InMemorySessionStore, SessionStore};

#[tokio::main]
async fn main() -> Result<()> {
    let _cleanup_guard = ProcessGroupCleanupGuard;

    init_tracing();
    let cli = Cli::parse(env::args().skip(1))?;
    if cli.help {
        print_usage();
        return Ok(());
    }
    run_daemon(
        cli.config,
        cli.mcp_config,
        cli.host,
        cli.port,
        cli.dashboard_host,
        cli.dashboard_port,
    )
    .await
}

async fn run_daemon(
    config_path: Option<PathBuf>,
    mcp_config_path: Option<PathBuf>,
    host: String,
    port: u16,
    dashboard_cli_host: Option<String>,
    dashboard_cli_port: Option<u16>,
) -> Result<()> {
    let config_path = resolve_config_path(config_path)?;
    xiaoo_shared::llm_secrets::init_on_demand_secret_provider(&config_path).with_context(|| {
        format!(
            "failed to initialize LLM secrets from {}",
            config_path.display()
        )
    })?;
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = DaemonConfig::load_with_mcp_config(
        &config_path,
        mcp_config_path.as_deref(),
        &workspace,
        dirs::home_dir().as_deref(),
    )?;
    let mcp_server_config = config.resolve_mcp_server_config()?;
    let hooker_config = config.app.hooker.clone();
    let bearer_auth = config.http_bearer_token()?.map(HttpBearerAuthConfig::new);
    let rate_limit = config.app.http.rate_limit.clone();
    let resolver = Arc::new(ConfiguredRuntimeResolver::from_config(&config).await?);
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let backend_manager = Arc::new(BackendManager::new());
    // Start the cross-process signal handler so backends owned by this
    // daemon that another process has marked for eviction get evicted
    // immediately upon receiving SIGUSR1.
    let handler_handle = backend_manager
        .clone()
        .start_signal_handler(session_store.clone());
    tokio::spawn(async move {
        handler_handle.await.ok();
    });
    let app = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        session_store.clone(),
        resolver,
        hooker_config,
        backend_manager.clone(),
    )?;
    let interaction_timeout_secs = config.interaction_timeout_secs();
    let session_service = app.session_service.clone();
    let session_control_plane = app.session_control_plane.clone();

    if let Some(telegram_config) = config.telegram_polling_config()? {
        spawn_telegram_polling_service(
            telegram_config,
            session_service.clone(),
            interaction_timeout_secs,
        )
        .context("failed to start telegram polling service")?;
    }

    if let Some(feishu_config) = config.feishu_config()? {
        if feishu_config.event_transport == FeishuEventTransport::Websocket {
            spawn_feishu_websocket_service(
                feishu_config,
                session_service.clone(),
                interaction_timeout_secs,
            )
            .context("failed to start Feishu websocket service")?;
        }
    }

    // ── Cron scheduler ──────────────────────────────────────────
    let cron_enabled = config.cron_section().is_some();
    let cron_scheduler = match config.resolve_cron_jobs() {
        Ok(jobs) if !jobs.is_empty() => {
            let global = config
                .cron_section()
                .expect("cron section must exist when jobs loaded");
            let total = jobs.len();
            let enabled_count = jobs.iter().filter(|j| j.enabled).count();
            if enabled_count > 0 {
                Some(Arc::new(CronScheduler::new(
                    jobs,
                    global.max_concurrent_jobs,
                    session_service.clone(),
                )))
            } else {
                tracing::info!(total, "no enabled cron jobs");
                None
            }
        }
        Ok(_) => {
            if cron_enabled {
                tracing::info!("cron section present but no jobs configured");
            }
            None
        }
        Err(error) => {
            tracing::error!(%error, "failed to load cron jobs, cron disabled");
            None
        }
    };

    let channel_runtimes = config.channel_runtimes()?;
    let mut router = if channel_runtimes.is_empty() {
        create_router_with_control_plane_and_auth(
            session_service.clone(),
            session_control_plane.clone(),
            bearer_auth,
            rate_limit.clone(),
        )
    } else {
        create_router_with_channel_runtimes_control_plane_and_timeout_and_auth(
            session_service.clone(),
            session_control_plane.clone(),
            channel_runtimes,
            interaction_timeout_secs,
            bearer_auth,
            rate_limit.clone(),
        )
        .map_err(anyhow::Error::new)
        .context("failed to create router with channel runtimes")?
    };
    if let Some(mcp_server_config) = mcp_server_config {
        router = router.merge(create_mcp_router(
            mcp_server_config,
            session_service.clone(),
            session_control_plane.clone(),
            session_store.clone(),
            rate_limit.clone(),
        ));
    }

    // Dashboard runs on its own listener so it never shares the runtime
    // API port (and its bearer auth). When `[http.dashboard].enabled = false`
    // is set in the config, `dashboard_port` resolves to `None` and no
    // dashboard server is started.
    if let Some(dash_addr) = spawn_dashboard_server(
        &config,
        dashboard_cli_host,
        dashboard_cli_port,
        session_store.clone(),
        backend_manager.clone(),
    )
    .await?
    {
        tracing::info!(%dash_addr, "dashboard ready at http://{dash_addr}");
        eprintln!("dashboard ready at http://{dash_addr}");
    } else {
        tracing::info!("dashboard disabled by config ([http.dashboard].enabled = false)");
    }

    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .with_context(|| format!("invalid listen address {host}:{port}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    tracing::info!(config = %config_path.display(), %addr, "starting rebuild daemon");
    let serve_result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum server exited unexpectedly");

    // Gracefully shutdown cron scheduler
    if let Some(scheduler) = cron_scheduler {
        scheduler.stop().await;
    }

    // Best-effort sandbox cleanup with a bounded timeout so a slow/stuck
    // provider delete call cannot keep the daemon alive indefinitely after a
    // shutdown signal. Matches the TUI exit path which also bounds remote
    // close to a few seconds.
    let shutdown_result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        backend_manager.shutdown_all(),
    )
    .await;
    match shutdown_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "failed to shutdown daemon backend manager");
        }
        Err(_) => {
            tracing::warn!(
                "daemon backend manager shutdown timed out after 10s; \
                 some sandboxes may linger and will be reclaimed lazily"
            );
        }
    }
    serve_result
}

/// Bind the dashboard listener. Starts at the preferred port (default
/// `28081`) and, if that port is already in use, walks forward up to 100
/// ports before giving up so a port conflict never blocks daemon startup.
/// Returns the resolved `SocketAddr` that was actually bound (which may
/// differ from the requested port), or `None` when the dashboard was
/// disabled via config.
async fn spawn_dashboard_server(
    config: &DaemonConfig,
    cli_host: Option<String>,
    cli_port: Option<u16>,
    session_store: Arc<dyn SessionStore>,
    backend_manager: Arc<BackendManager>,
) -> Result<Option<SocketAddr>> {
    let Some(requested_port) = config.dashboard_port(cli_port) else {
        return Ok(None);
    };
    let host = config.dashboard_host(cli_host);

    let max_attempts: u16 = 100;
    let mut current_port = requested_port;
    let listener = loop {
        let Ok(addr) = format!("{host}:{current_port}").parse::<SocketAddr>() else {
            bail!("invalid dashboard listen address {host}:{current_port}");
        };
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => break listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                let attempt = current_port.wrapping_sub(requested_port) + 1;
                if attempt >= max_attempts {
                    bail!(
                        "dashboard port {requested_port} and the next {} ports are all in use; \
                         set [http.dashboard].port or pass --dashboard-port to pick a free one",
                        max_attempts - 1
                    );
                }
                tracing::warn!(
                    "dashboard port {current_port} in use, trying {}",
                    current_port.wrapping_add(1)
                );
                current_port = current_port.wrapping_add(1);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("failed to bind dashboard {addr}"));
            }
        }
    };

    let bound_addr = listener
        .local_addr()
        .with_context(|| "failed to read bound dashboard address")?;
    let router = dashboard_router(DashboardState::new(session_store, backend_manager));
    tokio::spawn(async move {
        let result = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = wait_for_shutdown_signal().await;
            })
            .await;
        if let Err(error) = result {
            tracing::warn!(error = %error, "dashboard server exited with error");
        }
    });
    Ok(Some(bound_addr))
}

/// Future that completes when the daemon receives a shutdown signal (SIGINT or
/// SIGTERM). Wired into [`axum::serve`] via `with_graceful_shutdown` so that
/// the HTTP server stops accepting new connections and the `shutdown_all`
/// cleanup path that follows actually runs — without this the OS default
/// action terminates the process immediately and owned sandboxes are never
/// deleted. Also emits the single "received <signal>" log line; secondary
/// listeners (dashboard) should use [`wait_for_shutdown_signal`] instead so a
/// single Ctrl+C does not produce duplicate log lines.
async fn shutdown_signal() {
    match wait_for_shutdown_signal().await {
        "SIGINT" => {
            tracing::info!("received SIGINT (Ctrl+C), initiating graceful shutdown")
        }
        "SIGTERM" => tracing::info!("received SIGTERM, initiating graceful shutdown"),
        _ => {}
    }
}

/// Same as [`shutdown_signal`] but silent — no "received <signal>" log line.
/// Used by secondary `axum::serve` listeners (e.g. the dashboard server) so
/// that only the primary daemon handler logs the signal; each call still
/// installs its own SIGINT/SIGTERM listener, which is supported by tokio.
/// Returns the signal kind as a `&'static str` (`"SIGINT"` / `"SIGTERM"`,
/// or `""` on the non-Unix error path where nothing useful was received).
async fn wait_for_shutdown_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        // `None` when the handler could not be installed; the SIGTERM branch
        // below stays pending forever in that case so SIGINT alone still
        // drives the shutdown.
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => Some(s),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to install SIGTERM handler; only SIGINT will trigger shutdown"
                );
                None
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = async {
                match &mut sigterm {
                    Some(s) => s.recv().await,
                    None => std::future::pending().await,
                }
            } => "SIGTERM",
        }
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => "SIGINT",
            Err(error) => {
                tracing::warn!(error = %error, "failed to listen for Ctrl+C");
                ""
            }
        }
    }
}

fn spawn_feishu_websocket_service(
    feishu_config: FeishuConfig,
    session_service: Arc<dyn xiaoo_shared::gateway::SessionService>,
    interaction_timeout_secs: u64,
) -> Result<()> {
    let runtime = build_feishu_runtime(feishu_config.clone()).map_err(anyhow::Error::new)?;
    let processor =
        ChannelRuntimeProcessor::with_timeout(session_service, interaction_timeout_secs);
    let service = FeishuWebsocketService::new(feishu_config).map_err(anyhow::Error::new)?;
    let handler: FeishuWebsocketMessageHandler = Arc::new(move |message| {
        let processor = processor.clone();
        let runtime = runtime.clone();
        Box::pin(async move {
            if let Err(error) = processor.process_message(runtime, message).await {
                tracing::warn!("failed to process Feishu websocket message: {error}");
            }
        }) as BoxFuture<'static, ()>
    });
    tracing::info!("starting Feishu websocket transport");
    tokio::spawn(async move {
        service.run_forever(handler).await;
    });
    Ok(())
}

fn spawn_telegram_polling_service(
    telegram_config: TelegramConfig,
    session_service: Arc<dyn xiaoo_shared::gateway::SessionService>,
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
        .unwrap_or_else(|_| EnvFilter::new("info,xiaoo_serverside=debug,xiaoo_shared=debug"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

struct Cli {
    config: Option<PathBuf>,
    mcp_config: Option<PathBuf>,
    host: String,
    port: u16,
    dashboard_host: Option<String>,
    dashboard_port: Option<u16>,
    help: bool,
}

impl Cli {
    fn parse<I>(args: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut config = None;
        let mut mcp_config = None;
        let mut host = "0.0.0.0".to_string();
        let mut port = 18080_u16;
        let mut dashboard_host: Option<String> = None;
        let mut dashboard_port: Option<u16> = None;
        let remaining = args.into_iter().collect::<Vec<_>>();
        let mut index = 0;
        while index < remaining.len() {
            match remaining[index].as_str() {
                "--help" | "-h" => {
                    return Ok(Self {
                        config,
                        mcp_config,
                        host,
                        port,
                        dashboard_host,
                        dashboard_port,
                        help: true,
                    });
                }
                "--config" => {
                    index += 1;
                    let value = remaining.get(index).context("missing value for --config")?;
                    config = Some(PathBuf::from(value));
                }
                "--mcp-config" => {
                    index += 1;
                    let value = remaining
                        .get(index)
                        .context("missing value for --mcp-config")?;
                    mcp_config = Some(PathBuf::from(value));
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
                "--dashboard-host" => {
                    index += 1;
                    let value = remaining
                        .get(index)
                        .context("missing value for --dashboard-host")?;
                    dashboard_host = Some(value.clone());
                }
                "--dashboard-port" => {
                    index += 1;
                    let value = remaining
                        .get(index)
                        .context("missing value for --dashboard-port")?;
                    dashboard_port = Some(
                        value
                            .parse()
                            .with_context(|| format!("invalid dashboard port `{value}`"))?,
                    );
                }
                other => bail!("unknown argument `{other}`"),
            }
            index += 1;
        }
        Ok(Self {
            config,
            mcp_config,
            host,
            port,
            dashboard_host,
            dashboard_port,
            help: false,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage: xiaoo-daemon [--config <path>] [--mcp-config <path>] [--host <host>] [--port <port>]\n\
         \x20                  [--dashboard-host <host>] [--dashboard-port <port>]\n\n\
         Defaults: --host 0.0.0.0 --port 18080\n\
         \x20         --dashboard-host 127.0.0.1 --dashboard-port 28081\n\n\
         Dashboard port auto-increments on conflict (28081, 28082, ...)."
    );
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use std::path::PathBuf;

    #[test]
    fn parses_daemon_arguments() {
        let cli = Cli::parse(
            [
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

        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
        assert_eq!(cli.host, "127.0.0.1");
        assert_eq!(cli.port, 18080);
        assert!(!cli.help);
    }

    #[test]
    fn parses_daemon_mcp_config_argument() {
        let cli = Cli::parse(
            ["--mcp-config", "/tmp/mcp.json"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("daemon should accept --mcp-config");

        assert_eq!(cli.mcp_config, Some(PathBuf::from("/tmp/mcp.json")));
    }

    #[test]
    fn daemon_defaults_to_port_18080() {
        let cli = Cli::parse(std::iter::empty::<String>()).expect("cli should parse with defaults");

        assert_eq!(cli.host, "0.0.0.0");
        assert_eq!(cli.port, 18080);
    }

    #[test]
    fn daemon_help_flag_does_not_require_config() {
        let cli =
            Cli::parse(["--help"].into_iter().map(str::to_string)).expect("cli should parse help");

        assert!(cli.help);
    }

    #[test]
    fn dashboard_arguments_are_optional_and_default_to_none() {
        let cli = Cli::parse(std::iter::empty::<String>()).expect("cli should parse with defaults");

        assert!(cli.dashboard_host.is_none());
        assert!(cli.dashboard_port.is_none());
    }

    #[test]
    fn parses_dashboard_host_and_port() {
        let cli = Cli::parse(
            ["--dashboard-host", "0.0.0.0", "--dashboard-port", "29000"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("cli should parse dashboard flags");

        assert_eq!(cli.dashboard_host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cli.dashboard_port, Some(29000));
    }

    #[test]
    fn dashboard_port_must_be_numeric() {
        let result = Cli::parse(
            ["--dashboard-port", "not-a-number"]
                .into_iter()
                .map(str::to_string),
        );
        assert!(result.is_err());
    }
}
