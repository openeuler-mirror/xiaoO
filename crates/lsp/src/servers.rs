use std::path::{Path, PathBuf};

use serde_json::Value;
use tracing::{info, warn};

use agent_types::lsp::LspError;

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
    pub id: &'static str,
    pub extensions: &'static [&'static str],
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub root_markers: &'static [&'static str],
    pub language_id: &'static str,
    pub initialization_options: Option<Value>,
    pub auto_install: AutoInstall,
}

/// Return the global bin directory where auto-installed LSP servers are stored.
/// Override with `XIAOO_BIN` env var; defaults to `~/.local/share/xiaoo/bin`.
pub fn global_bin_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XIAOO_BIN") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    home.join(".local").join("share").join("xiaoo").join("bin")
}

/// Search PATH (plus `global_bin_dir()`) for `cmd`. Returns the first match.
pub fn which(cmd: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let extra = global_bin_dir();
    std::env::split_paths(&path_var)
        .chain(std::iter::once(extra))
        .flat_map(|dir| {
            let candidate = dir.join(cmd);
            #[cfg(windows)]
            let with_exe = dir.join(format!("{}.exe", cmd));
            #[cfg(not(windows))]
            let candidates: Vec<PathBuf> = vec![candidate];
            #[cfg(windows)]
            let candidates: Vec<PathBuf> = vec![candidate, with_exe];
            candidates
        })
        .find(|p| p.is_file())
}

/// Resolve the binary path for `config`: check PATH/global-bin first, then try auto-install.
pub async fn resolve_binary(config: &ServerConfig) -> Result<PathBuf, LspError> {
    if let Some(path) = which(config.command) {
        return Ok(path);
    }
    info!(
        server = config.id,
        command = config.command,
        "binary not found in PATH, attempting auto-install"
    );
    match try_auto_install(config).await {
        Ok(path) => {
            info!(server = config.id, ?path, "auto-install succeeded");
            Ok(path)
        }
        Err(msg) => {
            warn!(server = config.id, %msg, "auto-install failed");
            Err(LspError::StartupFailed(msg))
        }
    }
}

async fn try_auto_install(config: &ServerConfig) -> Result<PathBuf, String> {
    let bin_dir = global_bin_dir();
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create bin dir: {e}"))?;

    match config.auto_install {
        AutoInstall::None => Err(format!(
            "'{}' not found in PATH. Please install it manually.",
            config.command
        )),

        AutoInstall::GoInstall { package } => {
            if which("go").is_none() {
                return Err(format!(
                    "'{}' not found and Go toolchain ('go') is required to auto-install it.",
                    config.command
                ));
            }
            info!(server = config.id, %package, "running: go install");
            let status = tokio::process::Command::new("go")
                .args(["install", package])
                .env("GOBIN", &bin_dir)
                .status()
                .await
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("go install {package} failed (exit {status})"));
            }
            which(config.command)
                .ok_or_else(|| format!("'{}' still not found after go install", config.command))
        }

        AutoInstall::PipInstall { package } => {
            let pip = which("pip3").or_else(|| which("pip"));
            let Some(pip) = pip else {
                return Err(format!(
                    "'{}' not found and 'pip' is required to auto-install it.",
                    config.command
                ));
            };
            info!(server = config.id, %package, pip = %pip.display(), "running: pip install --user");
            let status = tokio::process::Command::new(&pip)
                .args(["install", "--user", package])
                .status()
                .await
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("pip install {package} failed (exit {status})"));
            }
            which(config.command)
                .ok_or_else(|| format!("'{}' still not found after pip install", config.command))
        }

        AutoInstall::NpmInstall { package } => {
            if which("npm").is_none() {
                return Err(format!(
                    "'{}' not found and 'npm' is required to auto-install it.",
                    config.command
                ));
            }
            info!(server = config.id, %package, "running: npm install -g");
            let status = tokio::process::Command::new("npm")
                .args(["install", "-g", package])
                .status()
                .await
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("npm install -g {package} failed (exit {status})"));
            }
            which(config.command)
                .ok_or_else(|| format!("'{}' still not found after npm install", config.command))
        }

        AutoInstall::CargoInstall { package } => {
            if which("cargo").is_none() {
                return Err(format!(
                    "'{}' not found and 'cargo' is required to auto-install it.",
                    config.command
                ));
            }
            let root_str = bin_dir.to_str().unwrap_or("/tmp");
            info!(server = config.id, %package, %root_str, "running: cargo install --root");
            let status = tokio::process::Command::new("cargo")
                .args(["install", "--root", root_str, package])
                .status()
                .await
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!("cargo install {package} failed (exit {status})"));
            }
            // cargo install --root puts the binary in <root>/bin/<name>
            let in_root_bin = bin_dir.join("bin").join(config.command);
            if in_root_bin.is_file() {
                return Ok(in_root_bin);
            }
            which(config.command).ok_or_else(|| {
                format!("'{}' still not found after cargo install", config.command)
            })
        }
    }
}

pub fn builtin_servers() -> Vec<ServerConfig> {
    vec![
        ServerConfig {
            id: "rust-analyzer",
            extensions: &["rs"],
            command: "rust-analyzer",
            args: &[],
            root_markers: &["Cargo.toml"],
            language_id: "rust",
            initialization_options: None,
            // Installed via `rustup component add rust-analyzer`; no generic auto-install.
            auto_install: AutoInstall::None,
        },
        ServerConfig {
            id: "gopls",
            extensions: &["go"],
            command: "gopls",
            args: &[],
            root_markers: &["go.mod"],
            language_id: "go",
            initialization_options: None,
            auto_install: AutoInstall::GoInstall {
                package: "golang.org/x/tools/gopls@latest",
            },
        },
        ServerConfig {
            id: "pyright",
            extensions: &["py"],
            command: "pyright-langserver",
            args: &["--stdio"],
            root_markers: &["pyproject.toml", "setup.py", "requirements.txt"],
            language_id: "python",
            initialization_options: Some(serde_json::json!({
                "python": { "pythonVersion": "3.11" }
            })),
            auto_install: AutoInstall::PipInstall { package: "pyright" },
        },
        ServerConfig {
            id: "typescript-language-server",
            extensions: &["ts", "tsx", "js", "jsx"],
            command: "typescript-language-server",
            args: &["--stdio"],
            root_markers: &["package.json", "tsconfig.json"],
            language_id: "typescript",
            initialization_options: None,
            auto_install: AutoInstall::NpmInstall {
                package: "typescript-language-server",
            },
        },
        ServerConfig {
            id: "clangd",
            extensions: &["c", "cc", "cpp", "cxx", "h", "hh", "hpp"],
            command: "clangd",
            args: &[],
            root_markers: &["compile_commands.json", "CMakeLists.txt", ".clangd"],
            language_id: "c",
            initialization_options: None,
            // Installed via system package manager (apt/brew/winget); no cross-platform auto-install.
            auto_install: AutoInstall::None,
        },
        ServerConfig {
            id: "zls",
            extensions: &["zig"],
            command: "zls",
            args: &[],
            root_markers: &["build.zig"],
            language_id: "zig",
            initialization_options: None,
            auto_install: AutoInstall::None,
        },
        ServerConfig {
            id: "lua-language-server",
            extensions: &["lua"],
            command: "lua-language-server",
            args: &[],
            root_markers: &[".luarc.json", ".luarc.jsonc"],
            language_id: "lua",
            initialization_options: None,
            auto_install: AutoInstall::None,
        },
    ]
}

/// Walk ancestors from `file` upward looking for any of `markers`.
/// Returns the first directory that contains one of the marker files/dirs.
/// Falls back to the file's parent directory.
pub fn find_root(file: &Path, markers: &[&str]) -> PathBuf {
    if let Some(parent) = file.parent() {
        for ancestor in parent.ancestors() {
            if markers.iter().any(|m| ancestor.join(m).exists()) {
                return ancestor.to_path_buf();
            }
        }
        parent.to_path_buf()
    } else {
        file.to_path_buf()
    }
}

/// Convert a filesystem path to an LSP file URI.
pub fn path_to_uri(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.starts_with('/') {
        format!("file://{}", s)
    } else {
        // Windows-style absolute path
        format!("file:///{}", s.replace('\\', "/"))
    }
}

/// Convert an LSP file URI back to a filesystem path string.
pub fn uri_to_path(uri: &str) -> String {
    if let Some(p) = uri.strip_prefix("file:///") {
        // Could be Windows (keep the drive letter) or Unix (add leading slash back)
        if p.contains(':') {
            p.to_string() // Windows: C:/...
        } else {
            format!("/{}", p)
        }
    } else if let Some(p) = uri.strip_prefix("file://") {
        p.to_string()
    } else {
        uri.to_string()
    }
}
