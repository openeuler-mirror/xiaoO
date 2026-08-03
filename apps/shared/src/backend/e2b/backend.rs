use agent_contracts::backend::{
    capability::{
        OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
    },
    BackendPath, OperationBackend, OperationBackendCapabilities, OperationError,
};
use async_trait::async_trait;
use base64::Engine;
use reqwest::header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::error::E2bFailure;
use super::exec::E2bExec;
use super::filesystem::E2bFileSystem;
use super::path::E2bPathResolver;
use super::search::E2bSearch;

pub(crate) const E2B_PROVIDER_KIND: &str = "e2b";
pub(crate) const DEFAULT_DOMAIN: &str = "e2b.app";
pub(crate) const DEFAULT_API_BASE: &str = "https://api.e2b.app";
pub(crate) const DEFAULT_TEMPLATE_ID: &str = "base";
pub(crate) const DEFAULT_ENVD_PORT: u16 = 49983;
pub(crate) const DEFAULT_WORKSPACE_ROOT: &str = "/home/user/workspace";
pub(crate) const DEFAULT_HOME_DIR: &str = "/home/user";
pub(crate) const DEFAULT_TEMP_ROOT: &str = "/tmp";
pub(crate) const DEFAULT_SHELL: &str = "/bin/sh";
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum E2bLifecycle {
    Active,
    ShuttingDown,
    Closed,
}

pub(crate) struct E2bBackendState {
    pub(crate) backend_id: String,
    pub(crate) api_base: String,
    pub(crate) api_key: String,
    pub(crate) sandbox_id: String,
    pub(crate) sandbox_domain: String,
    pub(crate) envd_access_token: Option<String>,
    pub(crate) envd_port: u16,
    pub(crate) envd_scheme: String,
    pub(crate) workspace_root: BackendPath,
    pub(crate) home_dir: Option<BackendPath>,
    pub(crate) temp_root: BackendPath,
    pub(crate) default_shell: Option<String>,
    pub(crate) username: Option<String>,
    /// When true, POST /files uses multipart (legacy envd); default is octet-stream.
    pub(crate) envd_file_upload_multipart: bool,
    pub(crate) http: reqwest::Client,
    pub(crate) lifecycle: Mutex<E2bLifecycle>,
}

pub struct E2bOperationBackend {
    backend_id: String,
    capabilities: OperationBackendCapabilities,
    state: Arc<E2bBackendState>,
    paths: E2bPathResolver,
    files: E2bFileSystem,
    search: E2bSearch,
    exec: E2bExec,
}

impl E2bOperationBackend {
    pub(crate) fn new(state: Arc<E2bBackendState>) -> Self {
        Self {
            backend_id: state.backend_id.clone(),
            capabilities: OperationBackendCapabilities {
                supports_atomic_write: false,
                supports_grep: true,
                supports_export_file: true,
                supports_lsp: false,
            },
            paths: E2bPathResolver::new(Arc::clone(&state)),
            files: E2bFileSystem::new(Arc::clone(&state)),
            search: E2bSearch::new(Arc::clone(&state)),
            exec: E2bExec::new(state.clone()),
            state,
        }
    }
}

impl E2bBackendState {
    pub(crate) fn ensure_active(&self) -> Result<(), OperationError> {
        let lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "e2b backend state lock poisoned".to_string(),
            })?;
        match *lifecycle {
            E2bLifecycle::Active => Ok(()),
            E2bLifecycle::ShuttingDown => Err(OperationError::Transport {
                message: format!("e2b backend {} is shutting down", self.backend_id),
            }),
            E2bLifecycle::Closed => Err(OperationError::Transport {
                message: format!("e2b backend {} is already closed", self.backend_id),
            }),
        }
    }

    fn begin_shutdown(&self) -> Result<bool, OperationError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "e2b backend state lock poisoned".to_string(),
            })?;
        match *lifecycle {
            E2bLifecycle::Active => {
                *lifecycle = E2bLifecycle::ShuttingDown;
                Ok(true)
            }
            E2bLifecycle::ShuttingDown | E2bLifecycle::Closed => Ok(false),
        }
    }

    fn finish_shutdown(&self) -> Result<(), OperationError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "e2b backend state lock poisoned".to_string(),
            })?;
        *lifecycle = E2bLifecycle::Closed;
        Ok(())
    }

    fn abort_shutdown(&self) -> Result<(), OperationError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "e2b backend state lock poisoned".to_string(),
            })?;
        if *lifecycle == E2bLifecycle::ShuttingDown {
            *lifecycle = E2bLifecycle::Active;
        }
        Ok(())
    }

    pub(crate) fn resolve_backend_path(
        &self,
        raw_path: &str,
        base: &BackendPath,
    ) -> Result<BackendPath, OperationError> {
        if raw_path == "~" || raw_path.starts_with("~/") {
            let home_dir = self
                .home_dir
                .as_ref()
                .ok_or_else(|| OperationError::Unsupported {
                    message: "home_dir is not configured".to_string(),
                })?;
            let suffix = raw_path.strip_prefix("~/").unwrap_or_default();
            return normalize_backend_path(Path::new(home_dir.0.as_str()).join(suffix).as_path());
        }

        let candidate = Path::new(raw_path);
        if candidate.is_absolute() {
            return normalize_backend_path(candidate);
        }
        normalize_backend_path(Path::new(base.0.as_str()).join(candidate).as_path())
    }

    pub(crate) fn envd_url(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!(
            "{}://{}/{}",
            self.envd_scheme,
            envd_host(
                self.envd_port,
                self.sandbox_id.as_str(),
                self.sandbox_domain.as_str()
            ),
            path
        )
    }

    pub(crate) fn platform_url(&self, path: &str) -> String {
        join_url(self.api_base.as_str(), path)
    }

    pub(crate) fn envd_request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self.http.request(method, self.envd_url(path));
        if let Some(token) = self
            .envd_access_token
            .as_deref()
            .filter(|token| !token.trim().is_empty())
        {
            request = request.header("X-Access-Token", token);
        }
        if let Some(username) = self.username.as_deref().filter(|name| !name.is_empty()) {
            let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{username}:"));
            if let Ok(value) = HeaderValue::from_str(format!("Basic {encoded}").as_str()) {
                request = request.header(AUTHORIZATION, value);
            }
        }
        request
    }

    pub(crate) async fn delete_sandbox(&self) -> Result<(), OperationError> {
        let started_at = Instant::now();
        let response = self
            .http
            .delete(self.platform_url(format!("/sandboxes/{}", self.sandbox_id).as_str()))
            .header("X-API-Key", self.api_key.as_str())
            .send()
            .await
            .map_err(|error| {
                let failure = E2bFailure::from_reqwest("failed to call e2b delete sandbox", &error);
                failure.log(
                    "delete_sandbox",
                    Some(self.sandbox_id.as_str()),
                    None,
                    started_at.elapsed().as_millis(),
                );
                OperationError::Transport {
                    message: failure.message,
                }
            })?;

        if response.status() == StatusCode::NO_CONTENT || response.status() == StatusCode::NOT_FOUND
        {
            return Ok(());
        }

        Err(http_error("delete e2b sandbox", response).await)
    }
}

#[async_trait]
impl OperationBackend for E2bOperationBackend {
    fn backend_id(&self) -> &str {
        self.backend_id.as_str()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        self.capabilities
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        &self.paths as &dyn OperationPathResolver
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files as &dyn OperationFileSystem
    }

    fn search(&self) -> &dyn OperationSearch {
        &self.search as &dyn OperationSearch
    }

    fn exec(&self) -> &dyn OperationExec {
        &self.exec as &dyn OperationExec
    }

    fn export(&self) -> &dyn OperationExport {
        &self.files as &dyn OperationExport
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        if !self.state.begin_shutdown()? {
            return Ok(());
        }
        match self.state.delete_sandbox().await {
            Ok(()) => {
                self.state.finish_shutdown()?;
                Ok(())
            }
            Err(error) => {
                self.state.abort_shutdown()?;
                Err(error)
            }
        }
    }
}

pub(crate) fn normalize_backend_path(path: &Path) -> Result<BackendPath, OperationError> {
    if !path.is_absolute() {
        return Err(OperationError::InvalidPath {
            message: format!("path must be absolute: {}", path.display()),
        });
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(OperationError::InvalidPath {
                        message: format!("path escapes root: {}", path.display()),
                    });
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
        }
    }

    let text = normalized
        .to_str()
        .ok_or_else(|| OperationError::InvalidPath {
            message: format!("path is not valid utf-8: {}", normalized.display()),
        })?;
    Ok(BackendPath(text.to_string()))
}

pub(crate) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) fn join_url(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

pub(crate) fn envd_host(port: u16, sandbox_id: &str, domain: &str) -> String {
    format!("{port}-{sandbox_id}.{domain}")
}

pub(crate) async fn http_error(context: &str, response: reqwest::Response) -> OperationError {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let message = parse_error_message(text.as_str()).unwrap_or(text);
    match status {
        StatusCode::NOT_FOUND => OperationError::NotFound { path: message },
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            OperationError::PermissionDenied { path: message }
        }
        _ => OperationError::Transport {
            message: format!("{context} failed with HTTP {status}: {message}"),
        },
    }
}

pub(crate) fn parse_error_message(text: &str) -> Option<String> {
    serde_json::from_str::<E2bError>(text)
        .ok()
        .map(|error| error.message)
        .filter(|message| !message.is_empty())
}

pub(crate) async fn connect_json<T: for<'de> Deserialize<'de>>(
    state: &E2bBackendState,
    path: &str,
    body: Value,
) -> Result<T, OperationError> {
    let started_at = Instant::now();
    let response = state
        .envd_request(Method::POST, path)
        .header("Connect-Protocol-Version", "1")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            let failure = E2bFailure::from_reqwest(
                format!("failed to call e2b envd {path}").as_str(),
                &error,
            );
            failure.log(
                "envd_json",
                Some(state.sandbox_id.as_str()),
                None,
                started_at.elapsed().as_millis(),
            );
            OperationError::Transport {
                message: failure.message,
            }
        })?;

    if !response.status().is_success() {
        return Err(http_error(path, response).await);
    }

    response
        .json::<T>()
        .await
        .map_err(|error| OperationError::Transport {
            message: format!("failed to decode e2b envd {path} response: {error}"),
        })
}

#[derive(Debug, Deserialize)]
struct E2bError {
    message: String,
}
