use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{info, warn};

use agent_contracts::backend::capability::exec::ExecRequest;
use agent_contracts::backend::capability::filesystem::ReadBytesRequest;
use agent_contracts::backend::BackendPath;
use agent_types::lsp::LspError;

use crate::host::LspEnv;

/// How to auto-install this server if the binary is not found in PATH.
#[derive(Debug, Clone)]
pub enum AutoInstall {
    /// No auto-install. The user must install manually.
    None,
    /// `go install <package>` — requires `go` in PATH; binary lands in `global_bin_dir()`.
    GoInstall { package: &'static str },
    /// `pip install --user <package>` — requires `pip3` or `pip` in PATH.
    PipInstall { package: &'static str },
    /// `npm install -g <package>` — requires `npm` in PATH.
    NpmInstall { package: &'static str },
    /// `cargo install <package>` — requires `cargo` in PATH; binary lands in `global_bin_dir()/bin/`.
    CargoInstall { package: &'static str },
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub id: String,
    pub extensions: Vec<String>,
    pub command: String,
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
    pub language_id: String,
    pub initialization_options: Option<Value>,
    pub auto_install: AutoInstall,
}

/// Resolve the binary path for `config`: check PATH/global-bin first, then try auto-install.
pub async fn resolve_binary(config: &ServerConfig, env: &dyn LspEnv) -> Result<PathBuf, LspError> {
    if let Some(path) = env.which(&config.command) {
        return Ok(path);
    }
    info!(
        server = config.id.as_str(),
        command = config.command.as_str(),
        "binary not found in PATH, attempting auto-install"
    );
    match try_auto_install(config, env).await {
        Ok(path) => {
            info!(server = config.id.as_str(), ?path, "auto-install succeeded");
            Ok(path)
        }
        Err(msg) => {
            warn!(server = config.id.as_str(), %msg, "auto-install failed");
            Err(LspError::StartupFailed(msg))
        }
    }
}

async fn try_auto_install(config: &ServerConfig, env: &dyn LspEnv) -> Result<PathBuf, String> {
    if matches!(config.auto_install, AutoInstall::None) {
        return Err(format!(
            "'{}' not found in PATH. Please install it manually.",
            config.command
        ));
    }
    let bin_dir = env.global_bin_dir();
    let bin_dir_bp = BackendPath::from_raw(bin_dir.to_string_lossy().into_owned());
    env.backend()
        .files()
        .create_dir_all(&bin_dir_bp)
        .await
        .map_err(|e| format!("failed to create bin dir: {e}"))?;

    match config.auto_install {
        AutoInstall::None => unreachable!("handled before preparing the install directory"),

        AutoInstall::GoInstall { package } => {
            if env.which("go").is_none() {
                return Err(format!(
                    "'{}' not found and Go toolchain ('go') is required to auto-install it.",
                    config.command
                ));
            }
            info!(server = config.id, %package, "running: go install");
            let result = env
                .backend()
                .exec()
                .exec(ExecRequest {
                    command: "go".to_string(),
                    args: vec!["install".to_string(), package.to_string()],
                    env: Some(vec![(
                        "GOBIN".to_string(),
                        bin_dir.to_string_lossy().into_owned(),
                    )]),
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            if result.exit_code != Some(0) {
                return Err(format!("go install {package} failed"));
            }
            env.which(&config.command)
                .ok_or_else(|| format!("'{}' still not found after go install", config.command))
        }

        AutoInstall::PipInstall { package } => {
            let pip = env.which("pip3").or_else(|| env.which("pip"));
            let Some(pip) = pip else {
                return Err(format!(
                    "'{}' not found and 'pip' is required to auto-install it.",
                    config.command
                ));
            };
            let pip_str = pip.to_string_lossy().into_owned();
            info!(server = config.id.as_str(), %package, pip = %pip_str, "running: pip install --user");
            let result = env
                .backend()
                .exec()
                .exec(ExecRequest {
                    command: pip_str,
                    args: vec![
                        "install".to_string(),
                        "--user".to_string(),
                        package.to_string(),
                    ],
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            if result.exit_code != Some(0) {
                return Err(format!("pip install {package} failed"));
            }
            env.which(&config.command)
                .ok_or_else(|| format!("'{}' still not found after pip install", config.command))
        }

        AutoInstall::NpmInstall { package } => {
            if env.which("npm").is_none() {
                return Err(format!(
                    "'{}' not found and 'npm' is required to auto-install it.",
                    config.command
                ));
            }
            let prefix = bin_dir.to_string_lossy().into_owned();
            info!(server = config.id.as_str(), %package, %prefix, "running: npm install --prefix");
            let result = env
                .backend()
                .exec()
                .exec(ExecRequest {
                    command: "npm".to_string(),
                    args: vec![
                        "install".to_string(),
                        "--prefix".to_string(),
                        prefix,
                        package.to_string(),
                    ],
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            if result.exit_code != Some(0) {
                return Err(format!("npm install {package} failed"));
            }
            env.which(&config.command)
                .ok_or_else(|| format!("'{}' still not found after npm install", config.command))
        }

        AutoInstall::CargoInstall { package } => {
            if env.which("cargo").is_none() {
                return Err(format!(
                    "'{}' not found and 'cargo' is required to auto-install it.",
                    config.command
                ));
            }
            let root_str = bin_dir.to_string_lossy().into_owned();
            info!(server = config.id.as_str(), %package, %root_str, "running: cargo install --root");
            let result = env
                .backend()
                .exec()
                .exec(ExecRequest {
                    command: "cargo".to_string(),
                    args: vec![
                        "install".to_string(),
                        "--root".to_string(),
                        root_str,
                        package.to_string(),
                    ],
                    ..Default::default()
                })
                .await
                .map_err(|e| e.to_string())?;
            if result.exit_code != Some(0) {
                return Err(format!("cargo install {package} failed"));
            }
            // cargo install --root puts the binary in <root>/bin/<name>
            let in_root_bin = bin_dir.join("bin").join(config.command.as_str());
            let in_root_bin_bp = BackendPath::from_raw(in_root_bin.to_string_lossy().into_owned());
            if env
                .backend()
                .files()
                .stat(&in_root_bin_bp)
                .await
                .map(|s| s.exists)
                .unwrap_or(false)
            {
                return Ok(in_root_bin);
            }
            env.which(&config.command)
                .ok_or_else(|| format!("'{}' still not found after cargo install", config.command))
        }
    }
}

pub fn builtin_servers() -> Vec<ServerConfig> {
    vec![
        ServerConfig {
            id: "rust-analyzer".to_string(),
            extensions: vec!["rs".to_string()],
            command: "rust-analyzer".to_string(),
            args: Vec::new(),
            root_markers: vec!["Cargo.toml".to_string()],
            language_id: "rust".to_string(),
            initialization_options: None,
            auto_install: AutoInstall::None,
        },
        ServerConfig {
            id: "gopls".to_string(),
            extensions: vec!["go".to_string()],
            command: "gopls".to_string(),
            args: Vec::new(),
            root_markers: vec!["go.mod".to_string()],
            language_id: "go".to_string(),
            initialization_options: None,
            auto_install: AutoInstall::GoInstall {
                package: "golang.org/x/tools/gopls@latest",
            },
        },
        ServerConfig {
            id: "pyright".to_string(),
            extensions: vec!["py".to_string()],
            command: "pyright-langserver".to_string(),
            args: vec!["--stdio".to_string()],
            root_markers: vec![
                "pyproject.toml".to_string(),
                "setup.py".to_string(),
                "requirements.txt".to_string(),
            ],
            language_id: "python".to_string(),
            initialization_options: Some(serde_json::json!({
                "python": { "pythonVersion": "3.11" }
            })),
            auto_install: AutoInstall::PipInstall { package: "pyright" },
        },
        ServerConfig {
            id: "typescript-language-server".to_string(),
            extensions: vec![
                "ts".to_string(),
                "tsx".to_string(),
                "js".to_string(),
                "jsx".to_string(),
            ],
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            root_markers: vec!["package.json".to_string(), "tsconfig.json".to_string()],
            language_id: "typescript".to_string(),
            initialization_options: None,
            auto_install: AutoInstall::NpmInstall {
                package: "typescript-language-server",
            },
        },
        ServerConfig {
            id: "clangd".to_string(),
            extensions: vec![
                "c".to_string(),
                "cc".to_string(),
                "cpp".to_string(),
                "cxx".to_string(),
                "h".to_string(),
                "hh".to_string(),
                "hpp".to_string(),
            ],
            command: "clangd".to_string(),
            args: Vec::new(),
            root_markers: vec![
                "compile_commands.json".to_string(),
                "CMakeLists.txt".to_string(),
                ".clangd".to_string(),
            ],
            language_id: "c".to_string(),
            initialization_options: None,
            auto_install: AutoInstall::None,
        },
        ServerConfig {
            id: "zls".to_string(),
            extensions: vec!["zig".to_string()],
            command: "zls".to_string(),
            args: Vec::new(),
            root_markers: vec!["build.zig".to_string()],
            language_id: "zig".to_string(),
            initialization_options: None,
            auto_install: AutoInstall::None,
        },
        ServerConfig {
            id: "lua-language-server".to_string(),
            extensions: vec!["lua".to_string()],
            command: "lua-language-server".to_string(),
            args: Vec::new(),
            root_markers: vec![".luarc.json".to_string(), ".luarc.jsonc".to_string()],
            language_id: "lua".to_string(),
            initialization_options: None,
            auto_install: AutoInstall::None,
        },
    ]
}

/// Walk ancestors from `file` upward looking for any of `markers`.
/// Uses `env.backend().files().stat()` so the check goes through the backend.
/// Falls back to the file's parent directory.
pub async fn find_root(file: &Path, markers: &[&str], env: &dyn LspEnv) -> PathBuf {
    if let Some(parent) = file.parent() {
        for ancestor in parent.ancestors() {
            for marker in markers {
                let candidate = ancestor.join(marker);
                let bp = BackendPath::from_raw(candidate.to_string_lossy().into_owned());
                if env
                    .backend()
                    .files()
                    .stat(&bp)
                    .await
                    .map(|s| s.exists)
                    .unwrap_or(false)
                {
                    return ancestor.to_path_buf();
                }
            }
        }
        parent.to_path_buf()
    } else {
        file.to_path_buf()
    }
}

/// Read a file's text content via the backend. Returns empty string on error.
pub async fn read_file(file: &Path, env: &dyn LspEnv) -> String {
    let bp = BackendPath::from_raw(file.to_string_lossy().into_owned());
    env.backend()
        .files()
        .read_bytes(ReadBytesRequest { path: bp })
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// Convert a filesystem path to an LSP file URI.
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.starts_with('/') {
        format!("file://{}", s)
    } else {
        format!("file:///{}", s.replace('\\', "/"))
    }
}

/// Convert an LSP file URI back to a filesystem path string.
pub fn uri_to_path(uri: &str) -> String {
    if let Some(p) = uri.strip_prefix("file:///") {
        if p.contains(':') {
            p.to_string()
        } else {
            format!("/{}", p)
        }
    } else if let Some(p) = uri.strip_prefix("file://") {
        p.to_string()
    } else {
        uri.to_string()
    }
}
