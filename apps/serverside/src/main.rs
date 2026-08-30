mod agent_management;
mod channels;
mod compact_management;
mod config_inspect;
mod config_schema;
mod config_validation;
mod cron;
mod daemon_config;
mod daemon_runtime;
mod hook_management;
mod httpserver;
mod lsp_management;
mod management_capabilities;
mod mcp_management;
mod mcp_server;
mod mcp_server_management;
mod memory_management;
mod model_management;
mod role_management;
mod skill_management;
mod tool_management;

use crate::channels::{
    build_feishu_runtime, build_telegram_runtime, FeishuConfig, FeishuEventTransport,
    FeishuWebsocketMessageHandler, FeishuWebsocketService, TelegramConfig,
    TelegramPollingMessageHandler, TelegramPollingService,
};
use crate::config_inspect::ConfigInspectOverrides;
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
use std::env;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;
use xiaoo_shared::backend::{BackendManager, ProcessGroupCleanupGuard};
use xiaoo_shared::gateway::{
    AppBootstrap, InMemorySessionStore, McpMemoryAutomation, SessionStore,
};

#[tokio::main]
async fn main() -> Result<()> {
    let _cleanup_guard = ProcessGroupCleanupGuard;

    init_tracing();
    let cli = Cli::parse(env::args().skip(1))?;
    if cli.help {
        print_usage();
        return Ok(());
    }
    match cli.action {
        CliAction::ValidateConfig => return validate_config(cli.config),
        CliAction::ConfigSchema => {
            println!("{}", config_schema::config_schema());
            return Ok(());
        }
        CliAction::ConfigProviders => {
            println!(
                "{}",
                serde_json::json!({
                    "schema_version": 1,
                    "providers": xiaoo_api::llm::provider_catalog(),
                })
            );
            return Ok(());
        }
        CliAction::ConfigInspect => {
            let config_path = resolve_config_path(cli.config)?;
            let report = config_inspect::inspect_config_file(
                &config_path,
                &ConfigInspectOverrides {
                    daemon_host: cli.host,
                    daemon_port: cli.port,
                    no_dashboard: cli.no_dashboard,
                    dashboard_host: cli.dashboard_host,
                    dashboard_port: cli.dashboard_port,
                    bearer_token_env: cli.bearer_token_env,
                },
            );
            println!("{}", serde_json::to_string(&report)?);
            return if report.valid {
                Ok(())
            } else {
                bail!("configuration inspection found validation errors")
            };
        }
        CliAction::ConfigTestModel => {
            let config_path = resolve_config_path(cli.config)?;
            let profile_id = cli
                .profile
                .as_deref()
                .context("config test-model requires --profile <id>")?;
            let report = model_management::test_model_connection(&config_path, profile_id).await?;
            println!("{}", serde_json::to_string(&report)?);
            return if report.success {
                Ok(())
            } else {
                bail!("model connection test failed")
            };
        }
        CliAction::ConfigModels => {
            let config_path = resolve_config_path(cli.config)?;
            let profile_id = cli
                .profile
                .as_deref()
                .context("config models requires --profile <id>")?;
            let report = model_management::list_model_catalog(&config_path, profile_id).await?;
            println!("{}", serde_json::to_string(&report)?);
            return if report.success {
                Ok(())
            } else {
                bail!("model catalog request failed")
            };
        }
        CliAction::ConfigRoles => {
            let config_path = resolve_config_path(cli.config)?;
            println!(
                "{}",
                serde_json::to_string(&role_management::role_catalog(&config_path)?)?
            );
            return Ok(());
        }
        CliAction::ConfigAgents => {
            let config_path = resolve_config_path(cli.config)?;
            let config = DaemonConfig::load_from(&config_path)?;
            println!(
                "{}",
                serde_json::to_string(&agent_management::agent_catalog(&config)?)?
            );
            return Ok(());
        }
        CliAction::ConfigTestAgent => {
            let config_path = resolve_config_path(cli.config)?;
            let agent_id = cli
                .agent
                .as_deref()
                .context("config test-agent requires --agent <id>")?;
            let config = DaemonConfig::load_from(&config_path)?;
            let report = agent_management::test_agent_startup(&config, agent_id).await;
            println!("{}", serde_json::to_string(&report)?);
            return Ok(());
        }
        CliAction::ConfigTools => {
            let config_path = resolve_config_path(cli.config)?;
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let config = DaemonConfig::load_with_mcp_config(
                &config_path,
                cli.mcp_config.as_deref(),
                &workspace,
                dirs::home_dir().as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string(&tool_management::tool_catalog(&config, &workspace).await?)?
            );
            return Ok(());
        }
        CliAction::ConfigCustomTools => {
            let config_path = resolve_config_path(cli.config)?;
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let config = DaemonConfig::load_from(&config_path)?;
            let supported = config
                .server_operation_backend()
                .is_none_or(|backend| backend.kind != "e2b");
            println!(
                "{}",
                serde_json::to_string(&xiaoo_shared::custom_tool_support::custom_tool_catalog(
                    Some(&workspace),
                    dirs::home_dir().as_deref(),
                    supported,
                ))?
            );
            return Ok(());
        }
        CliAction::ConfigRenderCustomTool => {
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let draft = serde_json::from_str::<
                xiaoo_shared::custom_tool_support::DeclarativeToolDraft,
            >(&input)
            .context("failed to parse custom tool draft JSON from stdin")?;
            println!(
                "{}",
                serde_json::to_string(&xiaoo_shared::custom_tool_support::render_custom_tool(
                    draft
                ))?
            );
            return Ok(());
        }
        CliAction::ConfigTestCustomTool => {
            let config_path = resolve_config_path(cli.config)?;
            let manifest = cli
                .manifest
                .as_deref()
                .context("config test-custom-tool requires --manifest <path>")?;
            let mut input = String::new();
            std::io::stdin().read_to_string(&mut input)?;
            let input = serde_json::from_str(&input)
                .context("failed to parse custom tool input JSON from stdin")?;
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let config = DaemonConfig::load_from(&config_path)?;
            let supported = config
                .server_operation_backend()
                .is_none_or(|backend| backend.kind != "e2b");
            let report = xiaoo_shared::custom_tool_support::test_custom_tool(
                &workspace,
                dirs::home_dir().as_deref(),
                manifest,
                input,
                supported,
                cli.allow_effects,
            )
            .await;
            println!("{}", serde_json::to_string(&report)?);
            return Ok(());
        }
        CliAction::ConfigSkills => {
            let config_path = resolve_config_path(cli.config)?;
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let config = DaemonConfig::load_from(&config_path)?;
            println!(
                "{}",
                serde_json::to_string(&skill_management::skill_catalog(&config, &workspace))?
            );
            return Ok(());
        }
        CliAction::ConfigHooks => {
            let config_path = resolve_config_path(cli.config)?;
            let config = DaemonConfig::load_from(&config_path)?;
            println!(
                "{}",
                serde_json::to_string(&hook_management::hook_catalog(&config)?)?
            );
            return Ok(());
        }
        CliAction::ConfigMcp => {
            let config_path = resolve_config_path(cli.config)?;
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let home = dirs::home_dir();
            let json_config_path = xiaoo_shared::mcp_support::resolve_json_config_path(
                cli.mcp_config.as_deref(),
                &workspace,
                home.as_deref(),
            );
            let config = DaemonConfig::load_with_mcp_config(
                &config_path,
                cli.mcp_config.as_deref(),
                &workspace,
                home.as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string(
                    &mcp_management::mcp_catalog(&config, json_config_path.as_deref()).await
                )?
            );
            return Ok(());
        }
        CliAction::ConfigLsp => {
            let config_path = resolve_config_path(cli.config)?;
            let config = DaemonConfig::load_from(&config_path)?;
            println!(
                "{}",
                serde_json::to_string(&lsp_management::lsp_catalog(&config))?
            );
            return Ok(());
        }
        CliAction::ConfigMcpServer => {
            let config_path = resolve_config_path(cli.config)?;
            let config = DaemonConfig::load_from(&config_path)?;
            println!(
                "{}",
                serde_json::to_string(&mcp_server_management::mcp_server_report(&config))?
            );
            return Ok(());
        }
        CliAction::ConfigMemory => {
            let config_path = resolve_config_path(cli.config)?;
            let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let home = dirs::home_dir();
            let config = DaemonConfig::load_with_mcp_config(
                &config_path,
                cli.mcp_config.as_deref(),
                &workspace,
                home.as_deref(),
            )?;
            println!(
                "{}",
                serde_json::to_string(&memory_management::memory_automation_report(&config).await)?
            );
            return Ok(());
        }
        CliAction::ConfigCompact => {
            let config_path = resolve_config_path(cli.config)?;
            let config = DaemonConfig::load_from(&config_path)?;
            println!(
                "{}",
                serde_json::to_string(&compact_management::compact_report(&config))?
            );
            return Ok(());
        }
        CliAction::ProtocolSchema => {
            println!("{}", xiaoo_shared::daemon_protocol::protocol_contract());
            return Ok(());
        }
        CliAction::Serve => {}
    }
    run_daemon(
        cli.config,
        cli.mcp_config,
        cli.host,
        cli.port,
        cli.dashboard_host,
        cli.dashboard_port,
        cli.no_dashboard,
        cli.ready_stdio,
        cli.bearer_token_env,
    )
    .await
}

fn validate_config(config_path: Option<PathBuf>) -> Result<()> {
    let config_path = resolve_config_path(config_path)?;
    let report = config_validation::validate_config_file(&config_path);
    println!("{}", serde_json::to_string(&report)?);
    if report.valid {
        Ok(())
    } else {
        bail!("configuration validation failed")
    }
}

async fn run_daemon(
    config_path: Option<PathBuf>,
    mcp_config_path: Option<PathBuf>,
    host: String,
    port: u16,
    dashboard_cli_host: Option<String>,
    dashboard_cli_port: Option<u16>,
    no_dashboard: bool,
    ready_stdio: bool,
    bearer_token_env: Option<String>,
) -> Result<()> {
    let config_path = resolve_config_path(config_path)?;
    xiaoo_shared::llm_secrets::inject_llm_secrets_into_env(&config_path).with_context(|| {
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
    let bearer_auth = match bearer_token_env {
        Some(env_name) => {
            let token = std::env::var(&env_name).with_context(|| {
                format!("bearer token environment variable `{env_name}` is not set")
            })?;
            if token.trim().is_empty() {
                bail!("bearer token environment variable `{env_name}` is empty");
            }
            Some(HttpBearerAuthConfig::new(token))
        }
        None => config.http_bearer_token()?.map(HttpBearerAuthConfig::new),
    };
    let rate_limit = config.app.http.rate_limit.clone();
    let resolver = Arc::new(ConfiguredRuntimeResolver::from_config(&config).await?);
    let session_store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let backend_manager = Arc::new(BackendManager::new());
    let memory_automation = match McpMemoryAutomation::connect(
        config.app.memory_automation.clone(),
        &config.app.mcp.servers,
    )
    .await
    {
        Ok(automation) => automation,
        Err(error) => {
            tracing::warn!(error = %error, "memory automation disabled after startup error");
            None
        }
    };
    let memory_automation_for_shutdown = memory_automation.clone();
    let serve_result = async {
        // Start the cross-process signal handler so backends owned by this
        // daemon that another process has marked for eviction get evicted
        // immediately upon receiving SIGUSR1.
        let handler_handle = backend_manager
            .clone()
            .start_signal_handler(session_store.clone());
        tokio::spawn(async move {
            handler_handle.await.ok();
        });
        let interaction_timeout_secs = config.interaction_timeout_secs();
        let app =
        AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
            session_store.clone(),
            resolver,
            hooker_config,
            backend_manager.clone(),
            memory_automation,
            Some(std::time::Duration::from_secs(interaction_timeout_secs)),
        )?;
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
        if no_dashboard {
            tracing::info!("dashboard disabled by --no-dashboard");
        } else {
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
        }

        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .with_context(|| format!("invalid listen address {host}:{port}"))?;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .with_context(|| format!("failed to bind {addr}"))?;
        let resolved_addr = listener
            .local_addr()
            .context("failed to resolve daemon listener address")?;
        tracing::info!(config = %config_path.display(), %resolved_addr, "starting xiaoo daemon");
        if ready_stdio {
            let ready = xiaoo_shared::daemon_protocol::response::DaemonReadyMessage {
                r#type: xiaoo_shared::daemon_protocol::response::DaemonReadyMessageType::Ready,
                service: xiaoo_shared::daemon_protocol::response::DaemonService::XiaooDaemon,
                host: resolved_addr.ip().to_string(),
                port: resolved_addr.port(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: xiaoo_shared::daemon_protocol::PROTOCOL_VERSION,
            };
            println!("{}", serde_json::to_string(&ready)?);
            std::io::stdout()
                .flush()
                .context("failed to flush daemon ready message")?;
        }
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
    .await;
    if let Some(automation) = memory_automation_for_shutdown {
        match tokio::time::timeout(std::time::Duration::from_secs(5), automation.close()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "failed to close daemon MCP memory automation");
            }
            Err(_) => {
                tracing::warn!("daemon MCP memory automation close timed out after 5 seconds");
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliAction {
    Serve,
    ValidateConfig,
    ConfigSchema,
    ConfigProviders,
    ConfigInspect,
    ConfigTestModel,
    ConfigModels,
    ConfigRoles,
    ConfigAgents,
    ConfigTestAgent,
    ConfigTools,
    ConfigCustomTools,
    ConfigRenderCustomTool,
    ConfigTestCustomTool,
    ConfigSkills,
    ConfigHooks,
    ConfigMcp,
    ConfigMcpServer,
    ConfigLsp,
    ConfigMemory,
    ConfigCompact,
    ProtocolSchema,
}

#[derive(Debug)]
struct Cli {
    action: CliAction,
    config: Option<PathBuf>,
    mcp_config: Option<PathBuf>,
    host: String,
    port: u16,
    dashboard_host: Option<String>,
    dashboard_port: Option<u16>,
    no_dashboard: bool,
    ready_stdio: bool,
    bearer_token_env: Option<String>,
    profile: Option<String>,
    agent: Option<String>,
    manifest: Option<PathBuf>,
    allow_effects: bool,
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
        let mut no_dashboard = false;
        let mut ready_stdio = false;
        let mut bearer_token_env = None;
        let mut profile = None;
        let mut agent = None;
        let mut manifest = None;
        let mut allow_effects = false;
        let mut remaining = args.into_iter().collect::<Vec<_>>();
        let action = match remaining.first().map(String::as_str) {
            Some("config") => {
                let action = match remaining.get(1).map(String::as_str) {
                    Some("validate") => CliAction::ValidateConfig,
                    Some("schema") => CliAction::ConfigSchema,
                    Some("providers") => CliAction::ConfigProviders,
                    Some("inspect") => CliAction::ConfigInspect,
                    Some("test-model") => CliAction::ConfigTestModel,
                    Some("models") => CliAction::ConfigModels,
                    Some("roles") => CliAction::ConfigRoles,
                    Some("agents") => CliAction::ConfigAgents,
                    Some("test-agent") => CliAction::ConfigTestAgent,
                    Some("tools") => CliAction::ConfigTools,
                    Some("custom-tools") => CliAction::ConfigCustomTools,
                    Some("render-custom-tool") => CliAction::ConfigRenderCustomTool,
                    Some("test-custom-tool") => CliAction::ConfigTestCustomTool,
                    Some("skills") => CliAction::ConfigSkills,
                    Some("hooks") => CliAction::ConfigHooks,
                    Some("mcp") => CliAction::ConfigMcp,
                    Some("mcp-server") => CliAction::ConfigMcpServer,
                    Some("lsp") => CliAction::ConfigLsp,
                    Some("memory") => CliAction::ConfigMemory,
                    Some("compact") => CliAction::ConfigCompact,
                    _ => bail!("unknown config command; expected `config validate`, `config schema`, `config providers`, `config inspect`, `config test-model`, `config models`, `config roles`, `config agents`, `config test-agent`, `config tools`, `config custom-tools`, `config render-custom-tool`, `config test-custom-tool`, `config skills`, `config hooks`, `config mcp`, `config mcp-server`, `config lsp`, `config memory`, or `config compact`"),
                };
                remaining.drain(0..2);
                action
            }
            Some("protocol") if remaining.get(1).map(String::as_str) == Some("schema") => {
                remaining.drain(0..2);
                CliAction::ProtocolSchema
            }
            _ => CliAction::Serve,
        };
        let mut index = 0;
        while index < remaining.len() {
            match remaining[index].as_str() {
                "--help" | "-h" => {
                    return Ok(Self {
                        action,
                        config,
                        mcp_config,
                        host,
                        port,
                        dashboard_host,
                        dashboard_port,
                        no_dashboard,
                        ready_stdio,
                        bearer_token_env,
                        profile,
                        agent,
                        manifest,
                        allow_effects,
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
                "--no-dashboard" => {
                    no_dashboard = true;
                }
                "--ready-stdio" => {
                    ready_stdio = true;
                }
                "--bearer-token-env" => {
                    index += 1;
                    let value = remaining
                        .get(index)
                        .context("missing value for --bearer-token-env")?;
                    if value.trim().is_empty() {
                        bail!("--bearer-token-env must not be empty");
                    }
                    bearer_token_env = Some(value.clone());
                }
                "--profile" => {
                    index += 1;
                    let value = remaining
                        .get(index)
                        .context("missing value for --profile")?;
                    if value.trim().is_empty() {
                        bail!("--profile must not be empty");
                    }
                    profile = Some(value.clone());
                }
                "--agent" => {
                    index += 1;
                    let value = remaining.get(index).context("missing value for --agent")?;
                    if value.trim().is_empty() {
                        bail!("--agent must not be empty");
                    }
                    agent = Some(value.clone());
                }
                "--manifest" => {
                    index += 1;
                    let value = remaining
                        .get(index)
                        .context("missing value for --manifest")?;
                    manifest = Some(PathBuf::from(value));
                }
                "--allow-effects" => {
                    allow_effects = true;
                }
                other => bail!("unknown argument `{other}`"),
            }
            index += 1;
        }
        Ok(Self {
            action,
            config,
            mcp_config,
            host,
            port,
            dashboard_host,
            dashboard_port,
            no_dashboard,
            ready_stdio,
            bearer_token_env,
            profile,
            agent,
            manifest,
            allow_effects,
            help: false,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage: xiaoo-daemon [--config <path>] [--mcp-config <path>] [--host <host>] [--port <port>]\n\
         \x20                  [--dashboard-host <host>] [--dashboard-port <port>]\n\
         \x20                  [--no-dashboard] [--ready-stdio] [--bearer-token-env <name>]\n\n\
         \x20     xiaoo-daemon config validate [--config <path>]\n\n\
         \x20     xiaoo-daemon config schema\n\n\
         \x20     xiaoo-daemon config providers\n\n\
         \x20     xiaoo-daemon config inspect [--config <path>] [daemon overrides]\n\n\
         \x20     xiaoo-daemon config test-model --profile <id> [--config <path>]\n\n\
         \x20     xiaoo-daemon config models --profile <id> [--config <path>]\n\n\
         \x20     xiaoo-daemon config roles [--config <path>]\n\n\
         \x20     xiaoo-daemon config agents [--config <path>]\n\n\
         \x20     xiaoo-daemon config test-agent --agent <id> [--config <path>]\n\n\
         \x20     xiaoo-daemon config tools [--config <path>] [--mcp-config <path>]\n\n\
         \x20     xiaoo-daemon config custom-tools [--config <path>]\n\n\
         \x20     xiaoo-daemon config render-custom-tool < draft.json\n\n\
         \x20     xiaoo-daemon config test-custom-tool --manifest <path> [--allow-effects] [--config <path>] < input.json\n\n\
         \x20     xiaoo-daemon config skills [--config <path>]\n\n\
         \x20     xiaoo-daemon config hooks [--config <path>]\n\n\
         \x20     xiaoo-daemon config mcp [--config <path>] [--mcp-config <path>]\n\n\
         \x20     xiaoo-daemon config mcp-server [--config <path>]\n\n\
         \x20     xiaoo-daemon config lsp [--config <path>]\n\n\
         \x20     xiaoo-daemon config memory [--config <path>] [--mcp-config <path>]\n\n\
         \x20     xiaoo-daemon config compact [--config <path>]\n\n\
         \x20     xiaoo-daemon protocol schema\n\n\
         Defaults: --host 0.0.0.0 --port 18080\n\
         \x20         --dashboard-host 127.0.0.1 --dashboard-port 28081\n\n\
         Dashboard port auto-increments on conflict (28081, 28082, ...)."
    );
}

#[cfg(test)]
mod tests {
    use super::{Cli, CliAction};
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
                "--no-dashboard",
                "--ready-stdio",
                "--bearer-token-env",
                "XIAOO_CLIENT_DAEMON_TOKEN",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("cli should parse");

        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
        assert_eq!(cli.host, "127.0.0.1");
        assert_eq!(cli.port, 18080);
        assert!(cli.no_dashboard);
        assert!(cli.ready_stdio);
        assert_eq!(
            cli.bearer_token_env.as_deref(),
            Some("XIAOO_CLIENT_DAEMON_TOKEN")
        );
        assert!(!cli.help);
        assert_eq!(cli.action, CliAction::Serve);
    }

    #[test]
    fn parses_config_validate_command() {
        let cli = Cli::parse(
            ["config", "validate", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config validate should parse");

        assert_eq!(cli.action, CliAction::ValidateConfig);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_schema_command() {
        let cli = Cli::parse(["config", "schema"].into_iter().map(str::to_string))
            .expect("config schema should parse");

        assert_eq!(cli.action, CliAction::ConfigSchema);
    }

    #[test]
    fn parses_config_providers_command() {
        let cli = Cli::parse(["config", "providers"].into_iter().map(str::to_string))
            .expect("config providers should parse");

        assert_eq!(cli.action, CliAction::ConfigProviders);
    }

    #[test]
    fn parses_config_inspect_with_startup_overrides() {
        let cli = Cli::parse(
            [
                "config",
                "inspect",
                "--config",
                "/tmp/demo.toml",
                "--no-dashboard",
                "--host",
                "127.0.0.1",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("config inspect should parse");

        assert_eq!(cli.action, CliAction::ConfigInspect);
        assert!(cli.no_dashboard);
        assert_eq!(cli.host, "127.0.0.1");
    }

    #[test]
    fn parses_config_test_model_command() {
        let cli = Cli::parse(
            [
                "config",
                "test-model",
                "--profile",
                "qwen",
                "--config",
                "/tmp/demo.toml",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("config test-model should parse");

        assert_eq!(cli.action, CliAction::ConfigTestModel);
        assert_eq!(cli.profile.as_deref(), Some("qwen"));
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_models_command() {
        let cli = Cli::parse(
            ["config", "models", "--profile", "qwen"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config models should parse");

        assert_eq!(cli.action, CliAction::ConfigModels);
        assert_eq!(cli.profile.as_deref(), Some("qwen"));
    }

    #[test]
    fn parses_config_roles_command() {
        let cli = Cli::parse(
            ["config", "roles", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config roles should parse");

        assert_eq!(cli.action, CliAction::ConfigRoles);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_agents_command() {
        let cli = Cli::parse(
            ["config", "agents", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config agents should parse");

        assert_eq!(cli.action, CliAction::ConfigAgents);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_test_agent_command() {
        let cli = Cli::parse(
            ["config", "test-agent", "--agent", "review"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config test-agent should parse");

        assert_eq!(cli.action, CliAction::ConfigTestAgent);
        assert_eq!(cli.agent.as_deref(), Some("review"));
    }

    #[test]
    fn parses_config_tools_command() {
        let cli = Cli::parse(
            ["config", "tools", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config tools should parse");

        assert_eq!(cli.action, CliAction::ConfigTools);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_custom_tools_command() {
        let cli = Cli::parse(
            ["config", "custom-tools", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config custom-tools should parse");

        assert_eq!(cli.action, CliAction::ConfigCustomTools);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_render_custom_tool_command() {
        let cli = Cli::parse(
            ["config", "render-custom-tool"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config render-custom-tool should parse");

        assert_eq!(cli.action, CliAction::ConfigRenderCustomTool);
    }

    #[test]
    fn parses_config_test_custom_tool_command() {
        let cli = Cli::parse(
            [
                "config",
                "test-custom-tool",
                "--manifest",
                "/tmp/echo.toml",
                "--allow-effects",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("config test-custom-tool should parse");

        assert_eq!(cli.action, CliAction::ConfigTestCustomTool);
        assert_eq!(cli.manifest, Some(PathBuf::from("/tmp/echo.toml")));
        assert!(cli.allow_effects);
    }

    #[test]
    fn parses_config_skills_command() {
        let cli = Cli::parse(
            ["config", "skills", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config skills should parse");

        assert_eq!(cli.action, CliAction::ConfigSkills);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_hooks_command() {
        let cli = Cli::parse(
            ["config", "hooks", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config hooks should parse");

        assert_eq!(cli.action, CliAction::ConfigHooks);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_mcp_command() {
        let cli = Cli::parse(
            [
                "config",
                "mcp",
                "--config",
                "/tmp/demo.toml",
                "--mcp-config",
                "/tmp/mcp.json",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .expect("config mcp should parse");

        assert_eq!(cli.action, CliAction::ConfigMcp);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
        assert_eq!(cli.mcp_config, Some(PathBuf::from("/tmp/mcp.json")));
    }

    #[test]
    fn parses_config_lsp_command() {
        let cli = Cli::parse(
            ["config", "lsp", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config lsp should parse");

        assert_eq!(cli.action, CliAction::ConfigLsp);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_mcp_server_command() {
        let cli = Cli::parse(
            ["config", "mcp-server", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config mcp-server should parse");

        assert_eq!(cli.action, CliAction::ConfigMcpServer);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_memory_command() {
        let cli = Cli::parse(
            ["config", "memory", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config memory should parse");

        assert_eq!(cli.action, CliAction::ConfigMemory);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_config_compact_command() {
        let cli = Cli::parse(
            ["config", "compact", "--config", "/tmp/demo.toml"]
                .into_iter()
                .map(str::to_string),
        )
        .expect("config compact should parse");

        assert_eq!(cli.action, CliAction::ConfigCompact);
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }

    #[test]
    fn parses_protocol_schema_command() {
        let cli = Cli::parse(["protocol", "schema"].into_iter().map(str::to_string))
            .expect("protocol schema should parse");

        assert_eq!(cli.action, CliAction::ProtocolSchema);
    }

    #[test]
    fn rejects_unknown_config_command() {
        let error = Cli::parse(["config", "unknown"].into_iter().map(str::to_string))
            .expect_err("unknown config command should fail");
        assert!(error.to_string().contains("`config test-model`"));
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
