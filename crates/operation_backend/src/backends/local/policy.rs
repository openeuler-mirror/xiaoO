use agent_contracts::backend::{OperationBackendBuildError, OperationError};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::backend::normalize_absolute_host_path;

#[derive(Debug, Clone)]
pub(crate) enum LocalIsolationConfig {
    None,
    MacosSeatbelt(MacosSeatbeltConfig),
}

#[derive(Debug, Clone)]
pub(crate) struct MacosSeatbeltConfig {
    pub(crate) allow_network: bool,
    readable_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct LocalBackendPolicy {
    isolation: LocalIsolationConfig,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum LocalIsolationOptions {
    MacosSeatbelt {
        #[serde(default = "default_allow_network")]
        allow_network: bool,
        #[serde(default)]
        readable_roots: Vec<String>,
        #[serde(default)]
        writable_roots: Vec<String>,
    },
}

fn default_allow_network() -> bool {
    true
}

impl LocalBackendPolicy {
    pub(crate) fn unrestricted() -> Self {
        Self {
            isolation: LocalIsolationConfig::None,
        }
    }

    pub(crate) fn from_isolation_options(
        options: Option<LocalIsolationOptions>,
        workspace_root: &Path,
        temp_root: &Path,
    ) -> Result<Self, OperationBackendBuildError> {
        let Some(options) = options else {
            return Ok(Self::unrestricted());
        };

        match options {
            LocalIsolationOptions::MacosSeatbelt {
                allow_network,
                readable_roots,
                writable_roots,
            } => {
                if !cfg!(target_os = "macos") {
                    return Err(OperationBackendBuildError::Unsupported {
                        message: "macos_seatbelt isolation is only supported on macOS".to_string(),
                    });
                }

                let mut readable = if readable_roots.is_empty() {
                    vec![normalize_build_path("workspace_root", workspace_root)?]
                } else {
                    normalize_build_roots("readable_roots", readable_roots)?
                };
                let writable = if writable_roots.is_empty() {
                    vec![
                        normalize_build_path("workspace_root", workspace_root)?,
                        normalize_build_path("temp_root", temp_root)?,
                    ]
                } else {
                    normalize_build_roots("writable_roots", writable_roots)?
                };

                for root in &writable {
                    if !readable.iter().any(|existing| existing == root) {
                        readable.push(root.clone());
                    }
                }

                Ok(Self {
                    isolation: LocalIsolationConfig::MacosSeatbelt(MacosSeatbeltConfig {
                        allow_network,
                        readable_roots: readable,
                        writable_roots: writable,
                    }),
                })
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn allow_network(&self) -> bool {
        match &self.isolation {
            LocalIsolationConfig::None => true,
            LocalIsolationConfig::MacosSeatbelt(config) => config.allow_network,
        }
    }

    pub(crate) fn check_read(&self, path: &Path) -> Result<(), OperationError> {
        let LocalIsolationConfig::MacosSeatbelt(config) = &self.isolation else {
            return Ok(());
        };
        let path = normalize_for_read(path)?;
        if config
            .readable_roots
            .iter()
            .any(|root| path_is_within(path.as_path(), root.as_path()))
        {
            return Ok(());
        }
        Err(OperationError::PermissionDenied {
            path: path.display().to_string(),
        })
    }

    pub(crate) fn check_write(&self, path: &Path) -> Result<(), OperationError> {
        let LocalIsolationConfig::MacosSeatbelt(config) = &self.isolation else {
            return Ok(());
        };
        let path = normalize_for_write(path)?;
        if config
            .writable_roots
            .iter()
            .any(|root| path_is_within(path.as_path(), root.as_path()))
        {
            return Ok(());
        }
        Err(OperationError::PermissionDenied {
            path: path.display().to_string(),
        })
    }

    pub(crate) fn check_exec_cwd(&self, path: &Path) -> Result<(), OperationError> {
        self.check_read(path)
    }

    pub(crate) fn seatbelt_profile(&self) -> Option<MacosSeatbeltProfile> {
        match &self.isolation {
            LocalIsolationConfig::None => None,
            LocalIsolationConfig::MacosSeatbelt(config) => {
                Some(MacosSeatbeltProfile::from_config(config))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_macos_seatbelt(
        readable_roots: Vec<PathBuf>,
        writable_roots: Vec<PathBuf>,
        allow_network: bool,
    ) -> Self {
        Self {
            isolation: LocalIsolationConfig::MacosSeatbelt(MacosSeatbeltConfig {
                allow_network,
                readable_roots,
                writable_roots,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MacosSeatbeltProfile {
    readable_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
    allow_network: bool,
}

impl MacosSeatbeltProfile {
    fn from_config(config: &MacosSeatbeltConfig) -> Self {
        Self {
            readable_roots: config.readable_roots.clone(),
            writable_roots: config.writable_roots.clone(),
            allow_network: config.allow_network,
        }
    }

    pub(crate) fn to_profile_text(&self) -> String {
        let mut lines = vec![
            "(version 1)".to_string(),
            "(deny default)".to_string(),
            "(allow process*)".to_string(),
            "(allow sysctl-read)".to_string(),
            "(allow file-read-metadata)".to_string(),
            "(allow file-read* (subpath \"/bin\"))".to_string(),
            "(allow file-read* (subpath \"/sbin\"))".to_string(),
            "(allow file-read* (subpath \"/usr\"))".to_string(),
            "(allow file-read* (subpath \"/System\"))".to_string(),
            "(allow file-read* (subpath \"/Library/Apple\"))".to_string(),
            "(allow file-read* (literal \"/dev/null\"))".to_string(),
            "(allow file-write* (literal \"/dev/null\"))".to_string(),
        ];

        for root in &self.readable_roots {
            lines.push(format!(
                "(allow file-read* (subpath {}))",
                profile_string(root)
            ));
        }
        for root in &self.writable_roots {
            lines.push(format!(
                "(allow file-read* (subpath {}))",
                profile_string(root)
            ));
            lines.push(format!(
                "(allow file-write* (subpath {}))",
                profile_string(root)
            ));
        }
        if self.allow_network {
            lines.push("(allow network*)".to_string());
        }

        lines.join("\n")
    }
}

fn normalize_build_roots(
    field_name: &str,
    roots: Vec<String>,
) -> Result<Vec<PathBuf>, OperationBackendBuildError> {
    roots
        .into_iter()
        .map(|root| normalize_build_path(field_name, Path::new(root.as_str())))
        .collect()
}

fn normalize_build_path(
    field_name: &str,
    path: &Path,
) -> Result<PathBuf, OperationBackendBuildError> {
    normalize_absolute_host_path(path)
        .and_then(|path| {
            std::fs::canonicalize(path.as_path())
                .map_err(|error| OperationError::InvalidPath {
                    message: format!("{}: {error}", path.display()),
                })
                .or(Ok(path))
        })
        .map_err(|error| OperationBackendBuildError::InvalidConfig {
            message: format!("{field_name}: {error}"),
        })
}

fn normalize_for_read(path: &Path) -> Result<PathBuf, OperationError> {
    let normalized = normalize_absolute_host_path(path)?;
    match std::fs::canonicalize(normalized.as_path()) {
        Ok(canonical) => normalize_absolute_host_path(canonical.as_path()),
        Err(_) => Ok(normalized),
    }
}

fn normalize_for_write(path: &Path) -> Result<PathBuf, OperationError> {
    let normalized = normalize_absolute_host_path(path)?;
    if let Ok(canonical) = std::fs::canonicalize(normalized.as_path()) {
        return normalize_absolute_host_path(canonical.as_path());
    }
    if let Some(parent) = normalized.parent() {
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            if let Some(name) = normalized.file_name() {
                return normalize_absolute_host_path(canonical_parent.join(name).as_path());
            }
        }
    }
    Ok(normalized)
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn profile_string(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> LocalBackendPolicy {
        LocalBackendPolicy::from_isolation_options(
            Some(LocalIsolationOptions::MacosSeatbelt {
                allow_network: false,
                readable_roots: vec!["/workspace".to_string()],
                writable_roots: vec!["/workspace/tmp".to_string()],
            }),
            Path::new("/workspace"),
            Path::new("/tmp"),
        )
        .unwrap_or_else(|_| LocalBackendPolicy {
            isolation: LocalIsolationConfig::MacosSeatbelt(MacosSeatbeltConfig {
                allow_network: false,
                readable_roots: vec![PathBuf::from("/workspace"), PathBuf::from("/workspace/tmp")],
                writable_roots: vec![PathBuf::from("/workspace/tmp")],
            }),
        })
    }

    #[test]
    fn workspace_read_is_allowed() {
        assert!(policy()
            .check_read(Path::new("/workspace/src/lib.rs"))
            .is_ok());
    }

    #[test]
    fn outside_workspace_read_is_denied() {
        assert!(matches!(
            policy().check_read(Path::new("/etc/passwd")),
            Err(OperationError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn escaped_path_is_denied_after_normalization() {
        assert!(matches!(
            policy().check_read(Path::new("/workspace/../etc/passwd")),
            Err(OperationError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn writable_root_is_also_readable() {
        assert!(policy()
            .check_read(Path::new("/workspace/tmp/result.txt"))
            .is_ok());
        assert!(policy()
            .check_write(Path::new("/workspace/tmp/result.txt"))
            .is_ok());
    }

    #[test]
    fn write_outside_writable_root_is_denied() {
        assert!(matches!(
            policy().check_write(Path::new("/workspace/src/lib.rs")),
            Err(OperationError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn profile_includes_roots_and_omits_network_when_disabled() {
        let text = policy().seatbelt_profile().unwrap().to_profile_text();
        assert!(text.contains("\"/workspace\""));
        assert!(text.contains("\"/workspace/tmp\""));
        assert!(!text.contains("(allow network*)"));
    }

    #[test]
    fn macos_seatbelt_defaults_to_allowing_network() {
        let options: LocalIsolationOptions =
            serde_json::from_value(serde_json::json!({"kind": "macos_seatbelt"})).unwrap();
        let LocalIsolationOptions::MacosSeatbelt { allow_network, .. } = options;

        assert!(allow_network);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn macos_seatbelt_is_unsupported_on_non_macos() {
        let error = LocalBackendPolicy::from_isolation_options(
            Some(LocalIsolationOptions::MacosSeatbelt {
                allow_network: false,
                readable_roots: vec![],
                writable_roots: vec![],
            }),
            Path::new("/workspace"),
            Path::new("/tmp"),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            OperationBackendBuildError::Unsupported { .. }
        ));
    }
}
