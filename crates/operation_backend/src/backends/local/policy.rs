use agent_contracts::backend::{
    OperationBackendBuildError, OperationError, SandboxPermissionCapability,
    SandboxPermissionGrantId, SandboxPermissionGrantRequest, SandboxPolicyDenial,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    grants: Arc<Mutex<GrantStore>>,
}

#[derive(Debug, Default)]
struct GrantStore {
    next_id: u64,
    grants: Vec<SandboxPermissionGrant>,
}

#[derive(Debug, Clone)]
struct SandboxPermissionGrant {
    id: SandboxPermissionGrantId,
    capability: SandboxPermissionCapability,
    path: PathBuf,
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
            grants: Arc::new(Mutex::new(GrantStore::default())),
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
                    grants: Arc::new(Mutex::new(GrantStore::default())),
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

    pub(crate) fn check_read(&self, path: &Path, operation: &str) -> Result<(), OperationError> {
        let LocalIsolationConfig::MacosSeatbelt(config) = &self.isolation else {
            return Ok(());
        };
        let path = normalize_for_read(path)?;
        if config
            .readable_roots
            .iter()
            .any(|root| path_is_within(path.as_path(), root.as_path()))
            || self.is_granted(path.as_path(), SandboxPermissionCapability::Read)?
        {
            return Ok(());
        }
        Err(sandbox_denial(
            operation,
            SandboxPermissionCapability::Read,
            path.as_path(),
        ))
    }

    pub(crate) fn check_write(&self, path: &Path, operation: &str) -> Result<(), OperationError> {
        let LocalIsolationConfig::MacosSeatbelt(config) = &self.isolation else {
            return Ok(());
        };
        let path = normalize_for_write(path)?;
        if config
            .writable_roots
            .iter()
            .any(|root| path_is_within(path.as_path(), root.as_path()))
            || self.is_granted(path.as_path(), SandboxPermissionCapability::Write)?
        {
            return Ok(());
        }
        Err(sandbox_denial(
            operation,
            SandboxPermissionCapability::Write,
            path.as_path(),
        ))
    }

    pub(crate) fn check_exec_cwd(&self, path: &Path) -> Result<(), OperationError> {
        let LocalIsolationConfig::MacosSeatbelt(config) = &self.isolation else {
            return Ok(());
        };
        let path = normalize_for_read(path)?;
        if config
            .readable_roots
            .iter()
            .any(|root| path_is_within(path.as_path(), root.as_path()))
            || self.is_granted(path.as_path(), SandboxPermissionCapability::ExecCwd)?
        {
            return Ok(());
        }
        Err(sandbox_denial(
            "bash",
            SandboxPermissionCapability::ExecCwd,
            path.as_path(),
        ))
    }

    pub(crate) fn seatbelt_profile(&self) -> Option<MacosSeatbeltProfile> {
        match &self.isolation {
            LocalIsolationConfig::None => None,
            LocalIsolationConfig::MacosSeatbelt(config) => {
                let grants = self.active_grants();
                if grants
                    .iter()
                    .any(|grant| grant.capability == SandboxPermissionCapability::ExecRuntime)
                {
                    return None;
                }
                Some(MacosSeatbeltProfile::from_config_and_grants(config, grants))
            }
        }
    }

    pub(crate) fn grant(
        &self,
        request: SandboxPermissionGrantRequest,
    ) -> Result<SandboxPermissionGrantId, OperationError> {
        let path = match request.denial.capability {
            SandboxPermissionCapability::Read | SandboxPermissionCapability::ExecCwd => {
                normalize_for_read(Path::new(request.denial.path.as_str()))?
            }
            SandboxPermissionCapability::ExecRuntime => PathBuf::new(),
            SandboxPermissionCapability::Write => write_grant_root(
                normalize_for_write(Path::new(request.denial.path.as_str()))?.as_path(),
            ),
        };
        let mut store = self.lock_grants()?;
        store.next_id += 1;
        let id = SandboxPermissionGrantId(store.next_id);
        store.grants.push(SandboxPermissionGrant {
            id,
            capability: request.denial.capability,
            path,
        });
        Ok(id)
    }

    pub(crate) fn revoke(&self, id: SandboxPermissionGrantId) -> Result<(), OperationError> {
        let mut store = self.lock_grants()?;
        store.grants.retain(|grant| grant.id != id);
        Ok(())
    }

    fn is_granted(
        &self,
        path: &Path,
        capability: SandboxPermissionCapability,
    ) -> Result<bool, OperationError> {
        let store = self.lock_grants()?;
        Ok(store
            .grants
            .iter()
            .any(|grant| grant.allows(capability) && path_is_within(path, grant.path.as_path())))
    }

    fn active_grants(&self) -> Vec<SandboxPermissionGrant> {
        self.grants
            .lock()
            .map(|store| store.grants.clone())
            .unwrap_or_default()
    }

    fn lock_grants(&self) -> Result<std::sync::MutexGuard<'_, GrantStore>, OperationError> {
        self.grants.lock().map_err(|_| OperationError::Transport {
            message: "local sandbox grant store lock poisoned".to_string(),
        })
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
            grants: Arc::new(Mutex::new(GrantStore::default())),
        }
    }
}

impl SandboxPermissionGrant {
    fn allows(&self, requested: SandboxPermissionCapability) -> bool {
        match (self.capability, requested) {
            (SandboxPermissionCapability::Write, SandboxPermissionCapability::Read) => true,
            (SandboxPermissionCapability::ExecCwd, SandboxPermissionCapability::Read) => true,
            (granted, requested) => granted == requested,
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
    fn from_config_and_grants(
        config: &MacosSeatbeltConfig,
        grants: Vec<SandboxPermissionGrant>,
    ) -> Self {
        let mut profile = Self {
            readable_roots: config.readable_roots.clone(),
            writable_roots: config.writable_roots.clone(),
            allow_network: config.allow_network,
        };
        for grant in grants {
            match grant.capability {
                SandboxPermissionCapability::Read | SandboxPermissionCapability::ExecCwd => {
                    push_unique_path(&mut profile.readable_roots, grant.path);
                }
                SandboxPermissionCapability::ExecRuntime => {}
                SandboxPermissionCapability::Write => {
                    push_unique_path(&mut profile.readable_roots, grant.path.clone());
                    push_unique_path(&mut profile.writable_roots, grant.path);
                }
            }
        }
        profile
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
                "(allow file-read* (literal {}))",
                profile_string(root)
            ));
            lines.push(format!(
                "(allow file-read* (subpath {}))",
                profile_string(root)
            ));
        }
        for root in &self.writable_roots {
            lines.push(format!(
                "(allow file-read* (literal {}))",
                profile_string(root)
            ));
            lines.push(format!(
                "(allow file-read* (subpath {}))",
                profile_string(root)
            ));
            lines.push(format!(
                "(allow file-write* (literal {}))",
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

fn write_grant_root(path: &Path) -> PathBuf {
    if path.is_dir() {
        return path.to_path_buf();
    }
    match path.parent() {
        Some(parent) if parent != Path::new(std::path::MAIN_SEPARATOR_STR) => parent.to_path_buf(),
        _ => path.to_path_buf(),
    }
}

fn profile_string(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn sandbox_denial(
    operation: &str,
    capability: SandboxPermissionCapability,
    path: &Path,
) -> OperationError {
    OperationError::SandboxPolicyDenied {
        denial: SandboxPolicyDenial {
            backend_id: "local".to_string(),
            isolation: "macos_seatbelt".to_string(),
            operation: operation.to_string(),
            capability,
            path: path.display().to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_contracts::backend::{
        SandboxPermissionGrantRequest, SandboxPermissionScope, SandboxPolicyDenial,
    };

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
            grants: Arc::new(Mutex::new(GrantStore::default())),
        })
    }

    #[test]
    fn workspace_read_is_allowed() {
        assert!(policy()
            .check_read(Path::new("/workspace/src/lib.rs"), "file_read")
            .is_ok());
    }

    #[test]
    fn outside_workspace_read_is_denied() {
        assert!(matches!(
            policy().check_read(Path::new("/etc/passwd"), "file_read"),
            Err(OperationError::SandboxPolicyDenied { .. })
        ));
    }

    #[test]
    fn escaped_path_is_denied_after_normalization() {
        assert!(matches!(
            policy().check_read(Path::new("/workspace/../etc/passwd"), "file_read"),
            Err(OperationError::SandboxPolicyDenied { .. })
        ));
    }

    #[test]
    fn writable_root_is_also_readable() {
        assert!(policy()
            .check_read(Path::new("/workspace/tmp/result.txt"), "file_read")
            .is_ok());
        assert!(policy()
            .check_write(Path::new("/workspace/tmp/result.txt"), "file_write")
            .is_ok());
    }

    #[test]
    fn write_outside_writable_root_is_denied() {
        assert!(matches!(
            policy().check_write(Path::new("/workspace/src/lib.rs"), "file_write"),
            Err(OperationError::SandboxPolicyDenied { .. })
        ));
    }

    #[test]
    fn granted_read_root_is_allowed() {
        let granted_policy = policy();
        let request = grant_request(
            SandboxPermissionCapability::Read,
            "/outside",
            SandboxPermissionScope::Session,
        );

        granted_policy.grant(request).unwrap();

        assert!(policy()
            .check_read(Path::new("/outside/secret.txt"), "file_read")
            .is_err());
        assert!(granted_policy
            .check_read(Path::new("/outside/secret.txt"), "file_read")
            .is_ok());
    }

    #[test]
    fn granted_write_root_is_also_readable_and_revocable() {
        let policy = policy();
        let id = policy
            .grant(grant_request(
                SandboxPermissionCapability::Write,
                "/outside/result.txt",
                SandboxPermissionScope::Once,
            ))
            .unwrap();

        assert!(policy
            .check_write(Path::new("/outside/result.txt"), "file_write")
            .is_ok());
        assert!(policy
            .check_read(Path::new("/outside/result.txt"), "file_read")
            .is_ok());

        policy.revoke(id).unwrap();

        assert!(matches!(
            policy.check_write(Path::new("/outside/result.txt"), "file_write"),
            Err(OperationError::SandboxPolicyDenied { .. })
        ));
    }

    #[test]
    fn profile_includes_granted_roots() {
        let policy = policy();
        policy
            .grant(grant_request(
                SandboxPermissionCapability::Read,
                "/granted",
                SandboxPermissionScope::Session,
            ))
            .unwrap();

        let text = policy.seatbelt_profile().unwrap().to_profile_text();

        assert!(text.contains("\"/granted\""));
    }

    #[test]
    fn exec_runtime_grant_disables_seatbelt_profile_for_exec() {
        let policy = policy();
        assert!(policy.seatbelt_profile().is_some());

        policy
            .grant(grant_request(
                SandboxPermissionCapability::ExecRuntime,
                "<runtime path not reported by macOS Seatbelt>",
                SandboxPermissionScope::Once,
            ))
            .unwrap();

        assert!(policy.seatbelt_profile().is_none());
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

    fn grant_request(
        capability: SandboxPermissionCapability,
        path: &str,
        scope: SandboxPermissionScope,
    ) -> SandboxPermissionGrantRequest {
        SandboxPermissionGrantRequest {
            denial: SandboxPolicyDenial {
                backend_id: "local".to_string(),
                isolation: "macos_seatbelt".to_string(),
                operation: "test".to_string(),
                capability,
                path: path.to_string(),
            },
            scope,
        }
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
