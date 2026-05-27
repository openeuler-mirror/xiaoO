use std::future::Future;
use std::sync::Arc;

use agent_contracts::backend::capability::exec::{ExecRequest, ExecResult, OperationExec};
use agent_contracts::backend::capability::export::{ExportFileRequest, OperationExport};
use agent_contracts::backend::capability::filesystem::{
    OperationFileSystem, ReadBytesRequest, TempPathRequest, WriteBytesOutcome, WriteBytesRequest,
};
use agent_contracts::backend::capability::path::OperationPathResolver;
use agent_contracts::backend::capability::search::{
    GlobRequest, GrepRequest, GrepResult, OperationSearch,
};
use agent_contracts::backend::{
    BackendPath, ExportedFileHandle, ExportedFileMeta, ExportedFileReader, OperationBackend,
    OperationBackendCapabilities, OperationError, OperationPermissionControl, PathStat,
    SandboxPermissionCapability, SandboxPermissionGrantRequest, SandboxPermissionScope,
    SandboxPolicyDenial, SharedExportedFileHandle,
};
use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

const ALLOW_ONCE: &str = "Allow once";
const ALLOW_SESSION: &str = "Allow for this session";
const DENY: &str = "Deny";

pub(crate) struct PermissionAwareOperationBackend {
    inner: Arc<dyn OperationBackend>,
    files: PermissionAwareFileSystem,
    search: PermissionAwareSearch,
    exec: PermissionAwareExec,
    export: PermissionAwareExport,
}

impl PermissionAwareOperationBackend {
    pub(crate) fn new(
        inner: Arc<dyn OperationBackend>,
        interaction: Arc<dyn InteractionHandle>,
    ) -> Self {
        Self {
            files: PermissionAwareFileSystem::new(Arc::clone(&inner), Arc::clone(&interaction)),
            search: PermissionAwareSearch::new(Arc::clone(&inner), Arc::clone(&interaction)),
            exec: PermissionAwareExec::new(Arc::clone(&inner), Arc::clone(&interaction)),
            export: PermissionAwareExport::new(Arc::clone(&inner), interaction),
            inner,
        }
    }
}

#[async_trait]
impl OperationBackend for PermissionAwareOperationBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        self.inner.capabilities()
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        self.inner.paths()
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files
    }

    fn search(&self) -> &dyn OperationSearch {
        &self.search
    }

    fn exec(&self) -> &dyn OperationExec {
        &self.exec
    }

    fn export(&self) -> &dyn OperationExport {
        &self.export
    }

    fn permission_control(&self) -> Option<&dyn OperationPermissionControl> {
        self.inner.permission_control()
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        self.inner.shutdown().await
    }
}

struct PermissionAwareFileSystem {
    inner: Arc<dyn OperationBackend>,
    interaction: Arc<dyn InteractionHandle>,
}

impl PermissionAwareFileSystem {
    fn new(inner: Arc<dyn OperationBackend>, interaction: Arc<dyn InteractionHandle>) -> Self {
        Self { inner, interaction }
    }
}

#[async_trait]
impl OperationFileSystem for PermissionAwareFileSystem {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let path = path.clone();
            async move { inner.files().stat(&path).await }
        })
        .await
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.files().read_bytes(request).await }
        })
        .await
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.files().write_bytes(request).await }
        })
        .await
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let path = path.clone();
            async move { inner.files().create_dir_all(&path).await }
        })
        .await
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.files().temp_path(request).await }
        })
        .await
    }
}

struct PermissionAwareSearch {
    inner: Arc<dyn OperationBackend>,
    interaction: Arc<dyn InteractionHandle>,
}

impl PermissionAwareSearch {
    fn new(inner: Arc<dyn OperationBackend>, interaction: Arc<dyn InteractionHandle>) -> Self {
        Self { inner, interaction }
    }
}

#[async_trait]
impl OperationSearch for PermissionAwareSearch {
    async fn glob(&self, request: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.search().glob(request).await }
        })
        .await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, OperationError> {
        run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.search().grep(request).await }
        })
        .await
    }
}

struct PermissionAwareExec {
    inner: Arc<dyn OperationBackend>,
    interaction: Arc<dyn InteractionHandle>,
}

impl PermissionAwareExec {
    fn new(inner: Arc<dyn OperationBackend>, interaction: Arc<dyn InteractionHandle>) -> Self {
        Self { inner, interaction }
    }
}

#[async_trait]
impl OperationExec for PermissionAwareExec {
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        let mut result = run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.exec().exec(request).await }
        })
        .await?;
        if let Some(denial) = runtime_seatbelt_denial(&result, &request) {
            result = retry_after_permission(&self.inner, &self.interaction, denial, || {
                let inner = Arc::clone(&self.inner);
                let request = request.clone();
                async move { inner.exec().exec(request).await }
            })
            .await?;
        }
        if is_silent_seatbelt_exec_denial(&result) {
            result = retry_after_permission(
                &self.inner,
                &self.interaction,
                silent_runtime_denial(self.inner.backend_id()),
                || {
                    let inner = Arc::clone(&self.inner);
                    let request = request.clone();
                    async move { inner.exec().exec(request).await }
                },
            )
            .await?;
        }
        annotate_seatbelt_stderr(&mut result);
        Ok(result)
    }
}

struct PermissionAwareExport {
    inner: Arc<dyn OperationBackend>,
    interaction: Arc<dyn InteractionHandle>,
}

impl PermissionAwareExport {
    fn new(inner: Arc<dyn OperationBackend>, interaction: Arc<dyn InteractionHandle>) -> Self {
        Self { inner, interaction }
    }
}

#[async_trait]
impl OperationExport for PermissionAwareExport {
    async fn export_file(
        &self,
        request: ExportFileRequest,
    ) -> Result<SharedExportedFileHandle, OperationError> {
        let handle = run_with_permission(&self.inner, &self.interaction, || {
            let inner = Arc::clone(&self.inner);
            let request = request.clone();
            async move { inner.export().export_file(request).await }
        })
        .await?;
        Ok(Arc::new(PermissionAwareExportedFileHandle {
            inner_backend: Arc::clone(&self.inner),
            interaction: Arc::clone(&self.interaction),
            inner_handle: handle,
        }))
    }
}

struct PermissionAwareExportedFileHandle {
    inner_backend: Arc<dyn OperationBackend>,
    interaction: Arc<dyn InteractionHandle>,
    inner_handle: SharedExportedFileHandle,
}

#[async_trait]
impl ExportedFileHandle for PermissionAwareExportedFileHandle {
    fn metadata(&self) -> &ExportedFileMeta {
        self.inner_handle.metadata()
    }

    async fn open_read(&self) -> Result<ExportedFileReader, OperationError> {
        run_with_permission(&self.inner_backend, &self.interaction, || {
            let handle = Arc::clone(&self.inner_handle);
            async move { handle.open_read().await }
        })
        .await
    }
}

async fn run_with_permission<T, F, Fut>(
    backend: &Arc<dyn OperationBackend>,
    interaction: &Arc<dyn InteractionHandle>,
    mut operation: F,
) -> Result<T, OperationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, OperationError>>,
{
    match operation().await {
        Ok(value) => Ok(value),
        Err(OperationError::SandboxPolicyDenied { denial }) => {
            retry_after_permission(backend, interaction, denial, operation).await
        }
        Err(error) => Err(error),
    }
}

async fn retry_after_permission<T, F, Fut>(
    backend: &Arc<dyn OperationBackend>,
    interaction: &Arc<dyn InteractionHandle>,
    denial: SandboxPolicyDenial,
    mut operation: F,
) -> Result<T, OperationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, OperationError>>,
{
    let scope = request_permission(interaction, &denial).await?;
    let Some(control) = backend.permission_control() else {
        return Err(OperationError::SandboxPolicyDenied { denial });
    };
    let grant_id = control.grant(SandboxPermissionGrantRequest {
        denial: denial.clone(),
        scope,
    })?;
    let result = operation().await;
    if scope == SandboxPermissionScope::Once {
        control.revoke(grant_id)?;
    }
    result
}

async fn request_permission(
    interaction: &Arc<dyn InteractionHandle>,
    denial: &agent_contracts::backend::SandboxPolicyDenial,
) -> Result<SandboxPermissionScope, OperationError> {
    let response = interaction
        .ask(&InteractionRequest::Choice {
            prompt: format!(
                "macOS Seatbelt blocked {}\n\n{} access requested:\n{}",
                denial.operation, denial.capability, denial.path
            ),
            options: vec![
                ALLOW_ONCE.to_string(),
                ALLOW_SESSION.to_string(),
                DENY.to_string(),
            ],
            allow_custom_input: false,
            source: None,
        })
        .await;

    match response {
        InteractionResponse::Choice { value: Some(value) } if value == ALLOW_ONCE => {
            Ok(SandboxPermissionScope::Once)
        }
        InteractionResponse::Choice { value: Some(value) } if value == ALLOW_SESSION => {
            Ok(SandboxPermissionScope::Session)
        }
        _ => Err(OperationError::SandboxPolicyDenied {
            denial: denial.clone(),
        }),
    }
}

fn annotate_seatbelt_stderr(result: &mut ExecResult) {
    let stderr = String::from_utf8_lossy(result.stderr.as_slice());
    if !stderr.contains("Operation not permitted") && !stderr.contains("operation not permitted") {
        return;
    }
    let prefix =
        b"Command failed because macOS Seatbelt blocked an operation inside the local sandbox.\n";
    if result.stderr.starts_with(prefix) {
        return;
    }
    let mut annotated = prefix.to_vec();
    annotated.extend_from_slice(result.stderr.as_slice());
    result.stderr = annotated;
}

fn runtime_seatbelt_denial(
    result: &ExecResult,
    request: &ExecRequest,
) -> Option<SandboxPolicyDenial> {
    let stderr = String::from_utf8_lossy(result.stderr.as_slice());
    let path = denied_path_from_stderr(stderr.as_ref())?;
    Some(SandboxPolicyDenial {
        backend_id: "local".to_string(),
        isolation: "macos_seatbelt".to_string(),
        operation: "bash".to_string(),
        capability: infer_exec_denial_capability(request),
        path,
    })
}

fn silent_runtime_denial(backend_id: &str) -> SandboxPolicyDenial {
    SandboxPolicyDenial {
        backend_id: backend_id.to_string(),
        isolation: "macos_seatbelt".to_string(),
        operation: "bash".to_string(),
        capability: SandboxPermissionCapability::ExecRuntime,
        path: "<runtime path not reported by macOS Seatbelt>".to_string(),
    }
}

fn denied_path_from_stderr(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .find_map(|line| denied_path_from_stderr_line(line))
}

fn denied_path_from_stderr_line(line: &str) -> Option<String> {
    let marker = if line.contains("Operation not permitted") {
        "Operation not permitted"
    } else if line.contains("operation not permitted") {
        "operation not permitted"
    } else {
        return None;
    };
    let before_marker = line.split(marker).next()?.trim();
    let before_marker = before_marker.trim_end_matches(':').trim();
    let candidate = before_marker
        .rsplit(": ")
        .find_map(clean_denied_path_candidate)?;
    if candidate.starts_with('/') {
        Some(candidate)
    } else {
        None
    }
}

fn clean_denied_path_candidate(value: &str) -> Option<String> {
    let candidate = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches(':')
        .trim();
    if candidate.starts_with('/') {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn infer_exec_denial_capability(request: &ExecRequest) -> SandboxPermissionCapability {
    let command = request.command.trim_start().to_ascii_lowercase();
    let writes_by_redirection = command.contains('>');
    let writes_by_program = [
        "touch ", "mkdir ", "rm ", "rmdir ", "mv ", "cp ", "install ", "tee ", "sed -i", "perl -pi",
    ]
    .iter()
    .any(|prefix| command.starts_with(prefix));
    if writes_by_redirection || writes_by_program {
        SandboxPermissionCapability::Write
    } else {
        SandboxPermissionCapability::Read
    }
}

fn is_silent_seatbelt_exec_denial(result: &ExecResult) -> bool {
    result.exit_code.is_none()
        && !result.timed_out
        && result.stdout.is_empty()
        && result.stderr.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cat_runtime_denial_path() {
        let stderr = "cat: /Users/me/.ssh/config: Operation not permitted\n";

        assert_eq!(
            denied_path_from_stderr(stderr).as_deref(),
            Some("/Users/me/.ssh/config")
        );
    }

    #[test]
    fn parses_shell_redirection_runtime_denial_path() {
        let stderr = "bash: /Users/me/out.txt: Operation not permitted\n";

        assert_eq!(
            denied_path_from_stderr(stderr).as_deref(),
            Some("/Users/me/out.txt")
        );
    }

    #[test]
    fn infers_write_for_redirection() {
        let request = ExecRequest {
            command: "echo hi > /Users/me/out.txt".to_string(),
            args: vec![],
            shell: Some("bash".to_string()),
            cwd: None,
            timeout_ms: None,
            env: None,
        };

        assert_eq!(
            infer_exec_denial_capability(&request),
            SandboxPermissionCapability::Write
        );
    }

    #[test]
    fn detects_silent_exec_denial() {
        let result = ExecResult {
            stdout: vec![],
            stderr: vec![],
            exit_code: None,
            timed_out: false,
        };

        assert!(is_silent_seatbelt_exec_denial(&result));
    }

    #[test]
    fn silent_runtime_denial_requests_exec_runtime() {
        let denial = silent_runtime_denial("local");

        assert_eq!(denial.capability, SandboxPermissionCapability::ExecRuntime);
        assert_eq!(denial.path, "<runtime path not reported by macOS Seatbelt>");
    }

    #[test]
    fn does_not_treat_timeout_as_silent_denial() {
        let result = ExecResult {
            stdout: vec![],
            stderr: vec![],
            exit_code: None,
            timed_out: true,
        };

        assert!(!is_silent_seatbelt_exec_denial(&result));
    }
}
