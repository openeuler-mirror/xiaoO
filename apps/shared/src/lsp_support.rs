// Config-vocabulary re-export: the app config layers (endside
// support/config, serverside daemon_config) must name these types for serde
// deserialization and to hold the registry handle, so they cannot be hidden
// behind coarse-grained functions. `AutoInstall` / `ServerConfig` stay
// directly constructible by the apps until a coarse-grained lsp builder
// absorbs them. `build_extra_server_configs` below reuses this `pub use`.
pub use lsp::{AutoInstall, LspServiceRegistry, ServerConfig};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub trait ExtraServerConfigView {
    fn id(&self) -> &str;
    fn extensions(&self) -> &[String];
    fn command(&self) -> &str;
    fn args(&self) -> &[String];
    fn root_markers(&self) -> &[String];
    fn language_id(&self) -> &str;
}

/// Convert user-supplied [`ExtraServerConfig`] entries to the lsp crate's
/// [`ServerConfig`] format, cloning the strings into owned storage.
pub fn build_extra_server_configs<T: ExtraServerConfigView>(
    extra_servers: &[T],
) -> Vec<ServerConfig> {
    extra_servers
        .iter()
        .map(|c| ServerConfig {
            id: c.id().to_string(),
            extensions: c.extensions().to_vec(),
            command: c.command().to_string(),
            args: c.args().to_vec(),
            root_markers: c.root_markers().to_vec(),
            language_id: c.language_id().to_string(),
            initialization_options: None,
            auto_install: AutoInstall::None,
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct LspCatalogReport {
    pub schema_version: u32,
    pub enabled: bool,
    pub servers: Vec<LspServerSummary>,
}

#[derive(Debug, Serialize)]
pub struct LspServerSummary {
    pub id: String,
    pub source: &'static str,
    pub extensions: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
    pub language_id: String,
    pub auto_install: &'static str,
    pub configured: bool,
    pub installed: bool,
    pub executable_path: Option<String>,
    pub startable: bool,
    pub running: bool,
}

#[derive(Debug, Serialize)]
pub struct LspActionReport {
    pub schema_version: u32,
    pub action: &'static str,
    pub server_id: String,
    pub success: bool,
    pub duration_ms: u128,
    pub installed: bool,
    pub executable_path: Option<String>,
    pub error_kind: Option<&'static str>,
}

pub fn lsp_catalog<T: ExtraServerConfigView>(
    enabled: bool,
    disabled_servers: &[String],
    extra_servers: &[T],
) -> LspCatalogReport {
    let disabled = disabled_servers
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut servers = lsp::servers::builtin_servers()
        .into_iter()
        .map(|server| server_summary(server, "builtin", enabled, &disabled))
        .collect::<Vec<_>>();
    servers.extend(
        build_extra_server_configs(extra_servers)
            .into_iter()
            .map(|server| server_summary(server, "custom", enabled, &disabled)),
    );
    servers.sort_by(|left, right| left.id.cmp(&right.id));
    LspCatalogReport {
        schema_version: 1,
        enabled,
        servers,
    }
}

pub async fn install_lsp_server<T: ExtraServerConfigView>(
    enabled: bool,
    disabled_servers: &[String],
    extra_servers: &[T],
    server_id: &str,
) -> LspActionReport {
    run_lsp_action(
        "install",
        enabled,
        disabled_servers,
        extra_servers,
        server_id,
        None,
    )
    .await
}

pub async fn test_lsp_server<T: ExtraServerConfigView>(
    enabled: bool,
    disabled_servers: &[String],
    extra_servers: &[T],
    server_id: &str,
    workspace: &Path,
) -> LspActionReport {
    run_lsp_action(
        "test_startup",
        enabled,
        disabled_servers,
        extra_servers,
        server_id,
        Some(workspace),
    )
    .await
}

async fn run_lsp_action<T: ExtraServerConfigView>(
    action: &'static str,
    enabled: bool,
    disabled_servers: &[String],
    extra_servers: &[T],
    server_id: &str,
    workspace: Option<&Path>,
) -> LspActionReport {
    let started = std::time::Instant::now();
    let configs = effective_server_configs(extra_servers);
    let config = configs
        .iter()
        .find(|config| config.id == server_id)
        .cloned();
    let Some(config) = config else {
        return action_failure(action, server_id, started, "unknown_server");
    };
    if !enabled || disabled_servers.iter().any(|id| id == server_id) {
        return action_failure(action, server_id, started, "disabled");
    }
    if workspace.is_some_and(|path| !path.is_dir()) {
        return action_failure(action, server_id, started, "workspace_unavailable");
    }

    let backend = operation_backend::local_lsp_backend();
    let env: Arc<dyn lsp::LspEnv> = Arc::new(lsp::LocalLspEnv::new(backend));
    let result = match workspace {
        None => lsp::servers::resolve_binary(&config, env.as_ref())
            .await
            .map(|_| ()),
        Some(workspace) => {
            let service = lsp::LspService::new_custom(vec![config.clone()], Arc::clone(&env));
            service.test_startup(server_id, workspace).await
        }
    };
    let executable = find_executable(&config.command);
    LspActionReport {
        schema_version: 1,
        action,
        server_id: server_id.to_string(),
        success: result.is_ok(),
        duration_ms: started.elapsed().as_millis(),
        installed: executable.is_some(),
        executable_path: executable.map(|path| path.display().to_string()),
        error_kind: result.err().map(|error| lsp_error_kind(&error)),
    }
}

fn action_failure(
    action: &'static str,
    server_id: &str,
    started: std::time::Instant,
    error_kind: &'static str,
) -> LspActionReport {
    LspActionReport {
        schema_version: 1,
        action,
        server_id: server_id.to_string(),
        success: false,
        duration_ms: started.elapsed().as_millis(),
        installed: false,
        executable_path: None,
        error_kind: Some(error_kind),
    }
}

fn lsp_error_kind(error: &lsp::LspError) -> &'static str {
    match error {
        lsp::LspError::NoServerForFile(_) => "no_server",
        lsp::LspError::StartupFailed(message) if message.contains("Please install") => {
            "manual_install_required"
        }
        lsp::LspError::StartupFailed(_) => "startup_failed",
        lsp::LspError::NotRunning => "not_running",
        lsp::LspError::PermanentlyFailed(_) => "permanently_failed",
        lsp::LspError::Timeout => "timeout",
        lsp::LspError::Io(_) => "io_error",
        lsp::LspError::Json(_) => "protocol_error",
        lsp::LspError::Rpc { .. } => "rpc_error",
    }
}

fn effective_server_configs<T: ExtraServerConfigView>(extra_servers: &[T]) -> Vec<ServerConfig> {
    let mut configs = build_extra_server_configs(extra_servers);
    configs.extend(lsp::servers::builtin_servers());
    configs
}

fn server_summary(
    server: ServerConfig,
    source: &'static str,
    globally_enabled: bool,
    disabled: &HashSet<&str>,
) -> LspServerSummary {
    let executable = find_executable(&server.command);
    let installed = executable.is_some();
    let configured = globally_enabled && !disabled.contains(server.id.as_str());
    LspServerSummary {
        id: server.id,
        source,
        extensions: server.extensions.clone(),
        command: server.command,
        args: server.args.clone(),
        root_markers: server.root_markers.clone(),
        language_id: server.language_id,
        auto_install: auto_install_name(&server.auto_install),
        configured,
        installed,
        executable_path: executable.map(|path| path.display().to_string()),
        startable: configured && installed,
        running: false,
    }
}

fn find_executable(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let extra = std::env::var_os("XIAOO_BIN")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/share/xiaoo/bin")));
    let user_bin = dirs::home_dir().map(|home| home.join(".local/bin"));
    std::env::split_paths(&path)
        .chain(extra.clone())
        .chain(extra.map(|directory| directory.join("bin")))
        .chain(user_bin)
        .flat_map(|directory| {
            let candidate = directory.join(command);
            #[cfg(windows)]
            let candidates = vec![candidate, directory.join(format!("{command}.exe"))];
            #[cfg(not(windows))]
            let candidates = vec![candidate];
            candidates
        })
        .find(|path| path.is_file())
}

fn auto_install_name(auto_install: &AutoInstall) -> &'static str {
    match auto_install {
        AutoInstall::None => "none",
        AutoInstall::GoInstall { .. } => "go",
        AutoInstall::PipInstall { .. } => "pip",
        AutoInstall::NpmInstall { .. } => "npm",
        AutoInstall::CargoInstall { .. } => "cargo",
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/shared/lsp_support_test.rs"]
mod catalog_tests;
