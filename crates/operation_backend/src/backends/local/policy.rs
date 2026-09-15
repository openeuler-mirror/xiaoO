use agent_contracts::backend::{
    OperationBackendBuildError, OperationError, SandboxPermissionCapability,
    SandboxPermissionGrantId, SandboxPermissionGrantRequest, SandboxPermissionScope,
    SandboxPolicyDenial,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::backend::normalize_absolute_host_path;

#[derive(Debug, Clone, Default)]
pub(crate) enum LocalIsolationConfig {
    #[default]
    None,
    MacosSeatbelt(PathIsolationConfig),
    LinuxBubblewrap(PathIsolationConfig),
    LinuxDynsandbox(LinuxDynsandboxOptions),
}

#[derive(Debug, Clone)]
pub(crate) struct LinuxDynsandboxOptions {
    roots: PathIsolationConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct PathIsolationConfig {
    pub(crate) allow_network: bool,
    readable_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
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
    LinuxBubblewrap {
        #[serde(default = "default_allow_network")]
        allow_network: bool,
        #[serde(default)]
        readable_roots: Vec<String>,
        #[serde(default)]
        writable_roots: Vec<String>,
    },
    LinuxDynsandbox {
        #[serde(default = "default_allow_network")]
        allow_network: bool,
        #[serde(default)]
        readable_roots: Vec<String>,
        #[serde(default)]
        writable_roots: Vec<String>,
        #[serde(default = "default_no_landlock")]
        no_landlock: bool,
    },
}

fn default_allow_network() -> bool {
    true
}

fn default_no_landlock() -> bool {
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

        return match options {
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
                Ok(Self {
                    isolation: LocalIsolationConfig::MacosSeatbelt(build_path_isolation_config(
                        allow_network,
                        readable_roots,
                        writable_roots,
                        workspace_root,
                        temp_root,
                    )?),
                    grants: Arc::new(Mutex::new(GrantStore::default())),
                })
            }
            LocalIsolationOptions::LinuxBubblewrap {
                allow_network,
                readable_roots,
                writable_roots,
            } => {
                if !cfg!(target_os = "linux") {
                    return Err(OperationBackendBuildError::Unsupported {
                        message: "linux_bubblewrap isolation is only supported on Linux"
                            .to_string(),
                    });
                }
                if !bubblewrap_available() {
                    return Err(OperationBackendBuildError::Unsupported {
                        message: "linux_bubblewrap isolation requires bubblewrap (bwrap) in PATH"
                            .to_string(),
                    });
                }
                Ok(Self {
                    isolation: LocalIsolationConfig::LinuxBubblewrap(build_path_isolation_config(
                        allow_network,
                        readable_roots,
                        writable_roots,
                        workspace_root,
                        temp_root,
                    )?),
                    grants: Arc::new(Mutex::new(GrantStore::default())),
                })
            }
            LocalIsolationOptions::LinuxDynsandbox {
                allow_network,
                readable_roots,
                writable_roots,
                no_landlock,
            } => {
                if !cfg!(target_os = "linux") {
                    return Err(OperationBackendBuildError::Unsupported {
                        message: "linux_dynsandbox isolation is only supported on Linux"
                            .to_string(),
                    });
                }
                if !linux_dynsandbox_available() {
                    return Err(OperationBackendBuildError::Unsupported {
                        message: "linux_dynsandbox isolation requires dyn-sandbox in PATH"
                            .to_string(),
                    });
                }
                let roots = build_path_isolation_config(
                    allow_network,
                    readable_roots,
                    writable_roots,
                    workspace_root,
                    temp_root,
                )?;
                tracing::info!(
                    "dyn-sandbox isolation enabled: allow_network={} no_landlock={} readable_roots={:?} writable_roots={:?}",
                    allow_network, no_landlock, roots.readable_roots, roots.writable_roots
                );
                Ok(Self {
                    isolation: LocalIsolationConfig::LinuxDynsandbox(LinuxDynsandboxOptions {
                        roots,
                    }),
                    grants: Arc::new(Mutex::new(GrantStore::default())),
                })
            }
        };
    }

    pub(crate) fn check_read(&self, path: &Path, operation: &str) -> Result<(), OperationError> {
        let Some((isolation_name, config)) = self.active_path_isolation() else {
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
            isolation_name,
            operation,
            SandboxPermissionCapability::Read,
            path.as_path(),
        ))
    }

    pub(crate) fn check_write(&self, path: &Path, operation: &str) -> Result<(), OperationError> {
        let Some((isolation_name, config)) = self.active_path_isolation() else {
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
            isolation_name,
            operation,
            SandboxPermissionCapability::Write,
            path.as_path(),
        ))
    }

    pub(crate) fn check_exec_cwd(&self, path: &Path) -> Result<(), OperationError> {
        let Some((isolation_name, config)) = self.active_path_isolation() else {
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
            isolation_name,
            "bash",
            SandboxPermissionCapability::ExecCwd,
            path.as_path(),
        ))
    }

    pub(crate) fn sandbox_denial_for_path(
        &self,
        path: &Path,
        capability: SandboxPermissionCapability,
        operation: &str,
    ) -> Result<Option<SandboxPolicyDenial>, OperationError> {
        match capability {
            SandboxPermissionCapability::Read => {
                let path = normalize_for_read(path)?;
                if !path.exists() || path == Path::new(std::path::MAIN_SEPARATOR_STR) {
                    return Ok(None);
                }
                sandbox_denial_from_result(self.check_read(path.as_path(), operation))
            }
            SandboxPermissionCapability::Write => {
                let path = normalize_for_write(path)?;
                if !is_authorizable_write_target(path.as_path()) {
                    return Ok(None);
                }
                sandbox_denial_from_result(self.check_write(path.as_path(), operation))
            }
            SandboxPermissionCapability::ExecCwd => {
                let path = normalize_for_read(path)?;
                if !path.is_dir() || path == Path::new(std::path::MAIN_SEPARATOR_STR) {
                    return Ok(None);
                }
                sandbox_denial_from_result(self.check_exec_cwd(path.as_path()))
            }
            SandboxPermissionCapability::ExecRuntime => Ok(None),
        }
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
            LocalIsolationConfig::LinuxBubblewrap(_) | LocalIsolationConfig::LinuxDynsandbox(_) => {
                None
            }
        }
    }

    /// Whether the isolation needs a piped stdin as a runtime control channel
    /// (e.g. dyn-sandbox's streaming AUTH flow, where `ALLOW`/`DENY` lines are
    /// written to stdin on demand).
    pub(crate) fn requires_stdin(&self) -> bool {
        matches!(self.isolation, LocalIsolationConfig::LinuxDynsandbox(_))
    }

    pub(crate) fn linux_dynsandbox_args(
        &self,
        cwd: &Path,
        extra: Option<&serde_json::Value>,
    ) -> Option<Vec<String>> {
        let LocalIsolationConfig::LinuxDynsandbox(options) = &self.isolation else {
            return None;
        };
        let grants = self.active_grants();
        if grants
            .iter()
            .any(|grant| grant.capability == SandboxPermissionCapability::ExecRuntime)
        {
            return None;
        }
        let mut readable = options.roots.readable_roots.clone();
        let mut writable = options.roots.writable_roots.clone();
        for grant in grants {
            match grant.capability {
                SandboxPermissionCapability::Read | SandboxPermissionCapability::ExecCwd => {
                    push_unique_path(&mut readable, grant.path);
                }
                SandboxPermissionCapability::ExecRuntime => {}
                SandboxPermissionCapability::Write => {
                    push_unique_path(&mut readable, grant.path.clone());
                    push_unique_path(&mut writable, grant.path);
                }
            }
        }

        let mut args = Vec::new();
        for root in &readable {
            if writable.iter().any(|writable| writable == root) {
                continue;
            }
            args.push("--mount".to_string());
            args.push(format!("{}:ro", root.display()));
        }
        for root in &writable {
            args.push("--mount".to_string());
            args.push(format!("{}:rw", root.display()));
        }
        args.push("-c".to_string());
        args.push(cwd.to_string_lossy().into_owned());
        append_linux_dynsandbox_rules(&mut args, extra);
        Some(args)
    }

    pub(crate) fn bubblewrap_args(&self, cwd: &Path) -> Option<Vec<String>> {
        match &self.isolation {
            LocalIsolationConfig::LinuxBubblewrap(config) => {
                let grants = self.active_grants();
                if grants
                    .iter()
                    .any(|grant| grant.capability == SandboxPermissionCapability::ExecRuntime)
                {
                    return None;
                }
                Some(LinuxBubblewrapProfile::from_config_and_grants(config, grants).to_args(cwd))
            }
            LocalIsolationConfig::None
            | LocalIsolationConfig::MacosSeatbelt(_)
            | LocalIsolationConfig::LinuxDynsandbox(_) => None,
        }
    }

    pub(crate) fn requires_exec_cwd(&self) -> bool {
        matches!(
            self.isolation,
            LocalIsolationConfig::LinuxBubblewrap(_) | LocalIsolationConfig::LinuxDynsandbox(_)
        )
    }

    pub(crate) fn grant(
        &self,
        request: SandboxPermissionGrantRequest,
    ) -> Result<SandboxPermissionGrantId, OperationError> {
        if request.denial.capability == SandboxPermissionCapability::ExecRuntime
            && request.scope != SandboxPermissionScope::Once
        {
            return Err(OperationError::Unsupported {
                message: "exec runtime sandbox bypass can only be granted for one retry"
                    .to_string(),
            });
        }

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

    fn active_path_isolation(&self) -> Option<(&'static str, &PathIsolationConfig)> {
        match &self.isolation {
            LocalIsolationConfig::None => None,
            LocalIsolationConfig::MacosSeatbelt(config) => Some(("macos_seatbelt", config)),
            LocalIsolationConfig::LinuxBubblewrap(config) => Some(("linux_bubblewrap", config)),
            LocalIsolationConfig::LinuxDynsandbox(options) => {
                Some(("linux_dynsandbox", &options.roots))
            }
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
        config: &PathIsolationConfig,
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

#[derive(Debug, Clone)]
pub(crate) struct LinuxBubblewrapProfile {
    readable_roots: Vec<PathBuf>,
    writable_roots: Vec<PathBuf>,
    allow_network: bool,
}

impl LinuxBubblewrapProfile {
    fn from_config_and_grants(
        config: &PathIsolationConfig,
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

    pub(crate) fn to_args(&self, cwd: &Path) -> Vec<String> {
        let mut builder = BubblewrapArgsBuilder::new();
        builder.arg("--die-with-parent");
        builder.arg("--unshare-user");
        builder.arg("--unshare-ipc");
        builder.arg("--unshare-pid");
        builder.arg("--unshare-uts");
        builder.arg("--unshare-cgroup-try");
        if !self.allow_network {
            builder.arg("--unshare-net");
        }
        builder.bind_runtime_roots();

        for root in &self.readable_roots {
            if self.writable_roots.iter().any(|writable| writable == root) {
                continue;
            }
            builder.ro_bind(root, root);
        }
        for root in &self.writable_roots {
            builder.rw_bind(root, root);
        }

        builder.ensure_directory(cwd);
        builder.arg("--chdir");
        builder.arg(cwd);
        builder.into_args()
    }
}

struct BubblewrapArgsBuilder {
    args: Vec<String>,
    dirs: Vec<PathBuf>,
}

impl BubblewrapArgsBuilder {
    fn new() -> Self {
        Self {
            args: Vec::new(),
            dirs: Vec::new(),
        }
    }

    fn arg(&mut self, value: impl BubblewrapArg) {
        self.args.push(value.into_arg());
    }

    fn bind_runtime_roots(&mut self) {
        for root in ["/usr", "/bin", "/sbin", "/lib", "/lib64"] {
            let path = Path::new(root);
            if path.exists() {
                self.ro_bind(path, path);
            }
        }
        for root in ["/proc", "/dev"] {
            let path = Path::new(root);
            if path.exists() {
                self.arg(if root == "/proc" { "--proc" } else { "--dev" });
                self.arg(path);
            }
        }
        for root in [
            "/etc/ld.so.cache",
            "/etc/ld.so.conf",
            "/etc/ld.so.conf.d",
            "/etc/nsswitch.conf",
            "/etc/passwd",
            "/etc/group",
            "/etc/hosts",
            "/etc/resolv.conf",
            "/etc/ssl",
            "/etc/ca-certificates",
            "/etc/pki",
            "/etc/localtime",
        ] {
            let path = Path::new(root);
            if path.exists() {
                self.ro_bind(path, path);
            }
        }
    }

    fn ro_bind(&mut self, source: &Path, dest: &Path) {
        self.ensure_bind_destination(source, dest);
        self.arg("--ro-bind");
        self.arg(source);
        self.arg(dest);
    }

    fn rw_bind(&mut self, source: &Path, dest: &Path) {
        self.ensure_bind_destination(source, dest);
        self.arg("--bind");
        self.arg(source);
        self.arg(dest);
    }

    fn ensure_bind_destination(&mut self, source: &Path, dest: &Path) {
        let dir_dest = if source.is_file() {
            dest.parent()
                .unwrap_or(Path::new(std::path::MAIN_SEPARATOR_STR))
        } else {
            dest
        };
        self.ensure_directory(dir_dest);
    }

    fn ensure_directory(&mut self, dest: &Path) {
        let mut current = PathBuf::new();
        for component in dest.components() {
            current.push(component.as_os_str());
            if current == Path::new(std::path::MAIN_SEPARATOR_STR) {
                continue;
            }
            if self.dirs.iter().any(|existing| existing == &current) {
                continue;
            }
            self.arg("--dir");
            self.arg(current.as_path());
            self.dirs.push(current.clone());
        }
    }

    fn into_args(self) -> Vec<String> {
        self.args
    }
}

trait BubblewrapArg {
    fn into_arg(self) -> String;
}

impl BubblewrapArg for &str {
    fn into_arg(self) -> String {
        self.to_string()
    }
}

impl BubblewrapArg for &Path {
    fn into_arg(self) -> String {
        self.to_string_lossy().into_owned()
    }
}

fn build_path_isolation_config(
    allow_network: bool,
    readable_roots: Vec<String>,
    writable_roots: Vec<String>,
    workspace_root: &Path,
    temp_root: &Path,
) -> Result<PathIsolationConfig, OperationBackendBuildError> {
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

    Ok(PathIsolationConfig {
        allow_network,
        readable_roots: readable,
        writable_roots: writable,
    })
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

fn is_authorizable_write_target(path: &Path) -> bool {
    if path == Path::new(std::path::MAIN_SEPARATOR_STR) {
        return false;
    }
    if path.exists() {
        return true;
    }
    path.parent()
        .filter(|parent| *parent != Path::new(std::path::MAIN_SEPARATOR_STR))
        .map(|parent| parent.exists())
        .unwrap_or(false)
}

fn sandbox_denial_from_result(
    result: Result<(), OperationError>,
) -> Result<Option<SandboxPolicyDenial>, OperationError> {
    match result {
        Ok(()) => Ok(None),
        Err(OperationError::SandboxPolicyDenied { denial }) => Ok(Some(denial)),
        Err(error) => Err(error),
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

fn append_linux_dynsandbox_rules(args: &mut Vec<String>, extra: Option<&serde_json::Value>) {
    if let Some(extra) = extra {
        tracing::info!("dyn-sandbox per-invocation rules: received extra = {extra}");
        // Rules are not yet translated into CLI args; wire format pending
        // plugin alignment.
    }
    let _ = args;
}

#[cfg(target_os = "linux")]
fn bubblewrap_available() -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("bwrap").is_file()))
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn bubblewrap_available() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn linux_dynsandbox_available() -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("dyn-sandbox").is_file()))
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn linux_dynsandbox_available() -> bool {
    false
}

fn sandbox_denial(
    isolation: &str,
    operation: &str,
    capability: SandboxPermissionCapability,
    path: &Path,
) -> OperationError {
    OperationError::SandboxPolicyDenied {
        denial: SandboxPolicyDenial {
            backend_id: "local".to_string(),
            isolation: isolation.to_string(),
            operation: operation.to_string(),
            capability,
            path: path.display().to_string(),
        },
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/operation_backend/backends/local/policy_test.rs"]
mod tests;
