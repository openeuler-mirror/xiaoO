use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

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
    SandboxPermissionCapability, SandboxPermissionGrantId, SandboxPermissionGrantRequest,
    SandboxPermissionScope, SandboxPolicyDenial, SharedExportedFileHandle,
};
use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

const ALLOW_ONCE: &str = "Allow once";
const ALLOW_SESSION: &str = "Allow for this session";
const ALLOW_SIMILAR_BASH_SESSION: &str = "Allow similar bash commands for this path";
const DENY: &str = "Deny";
const MAX_SANDBOX_PERMISSION_RETRIES: usize = 6;

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
        let bash_rules = Arc::new(BashSandboxApprovalRules::default());
        Self {
            files: PermissionAwareFileSystem::new(Arc::clone(&inner), Arc::clone(&interaction)),
            search: PermissionAwareSearch::new(Arc::clone(&inner), Arc::clone(&interaction)),
            exec: PermissionAwareExec::new(
                Arc::clone(&inner),
                Arc::clone(&interaction),
                Arc::clone(&bash_rules),
            ),
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
    bash_rules: Arc<BashSandboxApprovalRules>,
}

impl PermissionAwareExec {
    fn new(
        inner: Arc<dyn OperationBackend>,
        interaction: Arc<dyn InteractionHandle>,
        bash_rules: Arc<BashSandboxApprovalRules>,
    ) -> Self {
        Self {
            inner,
            interaction,
            bash_rules,
        }
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
        let mut retries = 0;
        loop {
            let denial = runtime_seatbelt_denial(&result).or_else(|| {
                is_silent_seatbelt_exec_denial(&result)
                    .then(|| silent_runtime_denial(self.inner.backend_id()))
            });
            let Some(denial) = denial else {
                break;
            };
            if retries >= MAX_SANDBOX_PERMISSION_RETRIES {
                return Err(OperationError::SandboxPolicyDenied { denial });
            }
            let context = PermissionRequestContext::Bash {
                command: request.command.clone(),
                rules: Arc::clone(&self.bash_rules),
            };
            result = retry_after_permission(
                &self.inner,
                &self.interaction,
                denial,
                Some(&context),
                || {
                    let inner = Arc::clone(&self.inner);
                    let request = request.clone();
                    async move { inner.exec().exec(request).await }
                },
            )
            .await?;
            retries += 1;
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
    let mut cleanup = PermissionGrantCleanup::default();
    let mut retries = 0;
    loop {
        match operation().await {
            Ok(value) => {
                cleanup.revoke_all();
                return Ok(value);
            }
            Err(OperationError::SandboxPolicyDenied { denial }) => {
                if retries >= MAX_SANDBOX_PERMISSION_RETRIES {
                    cleanup.revoke_all();
                    return Err(OperationError::SandboxPolicyDenied { denial });
                }
                if let Err(error) =
                    grant_after_permission(backend, interaction, &denial, None, &mut cleanup).await
                {
                    cleanup.revoke_all();
                    return Err(error);
                }
                retries += 1;
            }
            Err(error) => {
                cleanup.revoke_all();
                return Err(error);
            }
        }
    }
}

async fn retry_after_permission<T, F, Fut>(
    backend: &Arc<dyn OperationBackend>,
    interaction: &Arc<dyn InteractionHandle>,
    denial: SandboxPolicyDenial,
    context: Option<&PermissionRequestContext>,
    mut operation: F,
) -> Result<T, OperationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, OperationError>>,
{
    let mut cleanup = PermissionGrantCleanup::default();
    let mut retries = 0;
    let mut current_denial = denial;
    loop {
        if let Err(error) =
            grant_after_permission(backend, interaction, &current_denial, context, &mut cleanup)
                .await
        {
            cleanup.revoke_all();
            return Err(error);
        }
        match operation().await {
            Ok(value) => {
                cleanup.revoke_all();
                return Ok(value);
            }
            Err(OperationError::SandboxPolicyDenied { denial }) => {
                if retries >= MAX_SANDBOX_PERMISSION_RETRIES {
                    cleanup.revoke_all();
                    return Err(OperationError::SandboxPolicyDenied { denial });
                }
                current_denial = denial;
                retries += 1;
            }
            Err(error) => {
                cleanup.revoke_all();
                return Err(error);
            }
        }
    }
}

async fn grant_after_permission(
    backend: &Arc<dyn OperationBackend>,
    interaction: &Arc<dyn InteractionHandle>,
    denial: &SandboxPolicyDenial,
    context: Option<&PermissionRequestContext>,
    cleanup: &mut PermissionGrantCleanup,
) -> Result<(), OperationError> {
    let scope = if context_matches_auto_approval(context, denial) {
        SandboxPermissionScope::Once
    } else {
        match tool::current_sandbox_permission_scope() {
            Some(scope) => scope,
            None => match request_permission(interaction, denial, context).await? {
                PermissionDecision::Grant(scope) => {
                    let _ = tool::approve_current_sandbox_permission(scope);
                    scope
                }
                PermissionDecision::AllowSimilarBashForSession => {
                    if let Some(PermissionRequestContext::Bash { command, rules }) = context {
                        rules.add(command, denial);
                    }
                    SandboxPermissionScope::Once
                }
            },
        }
    };
    let grant_scope = if denial.capability == SandboxPermissionCapability::ExecRuntime {
        SandboxPermissionScope::Once
    } else {
        scope
    };
    let Some(control) = backend.permission_control() else {
        return Err(OperationError::SandboxPolicyDenied {
            denial: denial.clone(),
        });
    };
    let grant_id = control.grant(SandboxPermissionGrantRequest {
        denial: denial.clone(),
        scope: grant_scope,
    })?;
    if grant_scope == SandboxPermissionScope::Once
        && !tool::register_once_sandbox_grant(Arc::clone(backend), grant_id)
    {
        cleanup.add(Arc::clone(backend), grant_id);
    }
    Ok(())
}

enum PermissionRequestContext {
    Bash {
        command: String,
        rules: Arc<BashSandboxApprovalRules>,
    },
}

enum PermissionDecision {
    Grant(SandboxPermissionScope),
    AllowSimilarBashForSession,
}

#[derive(Default)]
struct BashSandboxApprovalRules {
    rules: Mutex<Vec<BashSandboxApprovalRule>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BashSandboxApprovalRule {
    command_prefix: String,
    granted_root: PathBuf,
}

impl BashSandboxApprovalRules {
    fn add(&self, command: &str, denial: &SandboxPolicyDenial) {
        let Some(rule) = BashSandboxApprovalRule::from_denial(command, denial) else {
            return;
        };
        if let Ok(mut rules) = self.rules.lock() {
            if !rules.iter().any(|existing| existing == &rule) {
                rules.push(rule);
            }
        }
    }

    fn matches(&self, command: &str, denial: &SandboxPolicyDenial) -> bool {
        if denial.capability == SandboxPermissionCapability::ExecRuntime {
            return false;
        }
        let path = Path::new(denial.path.as_str());
        if !path.is_absolute() {
            return false;
        }
        self.rules
            .lock()
            .map(|rules| {
                rules.iter().any(|rule| {
                    command.starts_with(rule.command_prefix.as_str())
                        && path_is_within(path, rule.granted_root.as_path())
                })
            })
            .unwrap_or(false)
    }
}

impl BashSandboxApprovalRule {
    fn from_denial(command: &str, denial: &SandboxPolicyDenial) -> Option<Self> {
        if denial.capability == SandboxPermissionCapability::ExecRuntime {
            return None;
        }
        let path = Path::new(denial.path.as_str());
        if !path.is_absolute() {
            return None;
        }
        let command_prefix = bash_command_prefix_for_denial(command, denial.path.as_str())?;
        let granted_root = path.parent()?.to_path_buf();
        Some(Self {
            command_prefix,
            granted_root,
        })
    }
}

fn context_matches_auto_approval(
    context: Option<&PermissionRequestContext>,
    denial: &SandboxPolicyDenial,
) -> bool {
    match context {
        Some(PermissionRequestContext::Bash { command, rules }) => rules.matches(command, denial),
        None => false,
    }
}

async fn request_permission(
    interaction: &Arc<dyn InteractionHandle>,
    denial: &SandboxPolicyDenial,
    context: Option<&PermissionRequestContext>,
) -> Result<PermissionDecision, OperationError> {
    let response = interaction
        .ask(&InteractionRequest::Choice {
            prompt: permission_prompt(denial, context),
            options: permission_options(denial, context),
            allow_custom_input: false,
            source: None,
        })
        .await;

    match response {
        InteractionResponse::Choice { value: Some(value) } if value == ALLOW_ONCE => {
            Ok(PermissionDecision::Grant(SandboxPermissionScope::Once))
        }
        InteractionResponse::Choice { value: Some(value) }
            if value == ALLOW_SESSION
                && denial.capability != SandboxPermissionCapability::ExecRuntime =>
        {
            Ok(PermissionDecision::Grant(SandboxPermissionScope::Session))
        }
        InteractionResponse::Choice { value: Some(value) }
            if value == ALLOW_SIMILAR_BASH_SESSION
                && can_offer_similar_bash_approval(denial, context) =>
        {
            Ok(PermissionDecision::AllowSimilarBashForSession)
        }
        _ => Err(OperationError::SandboxPolicyDenied {
            denial: denial.clone(),
        }),
    }
}

fn permission_prompt(
    denial: &SandboxPolicyDenial,
    context: Option<&PermissionRequestContext>,
) -> String {
    let tool_name = tool::current_tool_name().unwrap_or_else(|| denial.operation.clone());
    let isolation = isolation_display_name(denial.isolation.as_str());
    if let Some(PermissionRequestContext::Bash { command, .. }) = context {
        return format!(
            "{isolation} blocked {tool_name}\n\nCommand:\n{command}\n\nBlocked operation: {} {}\n{}",
            denial.capability, denial.operation, denial.path
        );
    }
    format!(
        "{isolation} blocked {tool_name}\n\nAllow this tool call to continue with additional sandbox permissions?\nBlocked operation: {} {}\n{}",
        denial.capability, denial.operation, denial.path
    )
}

fn permission_options(
    denial: &SandboxPolicyDenial,
    context: Option<&PermissionRequestContext>,
) -> Vec<String> {
    if denial.capability == SandboxPermissionCapability::ExecRuntime {
        return vec![ALLOW_ONCE.to_string(), DENY.to_string()];
    }
    let mut options = vec![ALLOW_ONCE.to_string(), ALLOW_SESSION.to_string()];
    if can_offer_similar_bash_approval(denial, context) {
        options.push(ALLOW_SIMILAR_BASH_SESSION.to_string());
    }
    options.push(DENY.to_string());
    options
}

fn can_offer_similar_bash_approval(
    denial: &SandboxPolicyDenial,
    context: Option<&PermissionRequestContext>,
) -> bool {
    match context {
        Some(PermissionRequestContext::Bash { command, .. }) => {
            BashSandboxApprovalRule::from_denial(command, denial).is_some()
        }
        None => false,
    }
}

fn bash_command_prefix_for_denial(command: &str, denied_path: &str) -> Option<String> {
    if let Some(index) = command.find(denied_path) {
        let prefix = command[..index].to_string();
        if !prefix.trim().is_empty() {
            return Some(prefix);
        }
    }
    let trimmed = command.trim_start();
    let first_len = trimmed
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index))
        .unwrap_or(trimmed.len());
    if first_len == 0 {
        return None;
    }
    let leading_len = command.len() - trimmed.len();
    let end = leading_len + first_len;
    let mut prefix = command[..end].to_string();
    prefix.push(' ');
    Some(prefix)
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

#[derive(Default)]
struct PermissionGrantCleanup {
    once_grants: Vec<(Arc<dyn OperationBackend>, SandboxPermissionGrantId)>,
}

impl PermissionGrantCleanup {
    fn add(&mut self, backend: Arc<dyn OperationBackend>, id: SandboxPermissionGrantId) {
        self.once_grants.push((backend, id));
    }

    fn revoke_all(&mut self) {
        for (backend, id) in self.once_grants.drain(..) {
            if let Some(control) = backend.permission_control() {
                let _ = control.revoke(id);
            }
        }
    }
}

fn annotate_seatbelt_stderr(result: &mut ExecResult) {
    let stderr = String::from_utf8_lossy(result.stderr.as_slice());
    if !stderr.contains("Operation not permitted") && !stderr.contains("operation not permitted") {
        return;
    }
    let prefix = b"Command failed because the local sandbox blocked an operation.\n";
    if result.stderr.starts_with(prefix) {
        return;
    }
    let mut annotated = prefix.to_vec();
    annotated.extend_from_slice(result.stderr.as_slice());
    result.stderr = annotated;
}

fn isolation_display_name(isolation: &str) -> &'static str {
    match isolation {
        "macos_seatbelt" => "macOS Seatbelt",
        "linux_bubblewrap" => "Linux Bubblewrap",
        _ => "Local sandbox",
    }
}

fn runtime_seatbelt_denial(result: &ExecResult) -> Option<SandboxPolicyDenial> {
    let stderr = String::from_utf8_lossy(result.stderr.as_slice());
    let path = denied_path_from_stderr(stderr.as_ref())?;
    Some(SandboxPolicyDenial {
        backend_id: "local".to_string(),
        isolation: "macos_seatbelt".to_string(),
        operation: "bash".to_string(),
        capability: SandboxPermissionCapability::Write,
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
    fn runtime_denial_uses_write_grant_to_cover_unknown_path_access() {
        let result = ExecResult {
            stdout: vec![],
            stderr: b"cat: /Users/me/.ssh/config: Operation not permitted\n".to_vec(),
            exit_code: Some(1),
            timed_out: false,
        };

        assert_eq!(
            runtime_seatbelt_denial(&result).map(|denial| denial.capability),
            Some(SandboxPermissionCapability::Write)
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
    fn exec_runtime_denial_only_offers_once_or_deny() {
        let denial = silent_runtime_denial("local");

        assert_eq!(
            permission_options(&denial, None),
            vec![ALLOW_ONCE.to_string(), DENY.to_string()]
        );
    }

    #[test]
    fn permission_prompt_uses_bubblewrap_display_name() {
        let denial = SandboxPolicyDenial {
            backend_id: "local".to_string(),
            isolation: "linux_bubblewrap".to_string(),
            operation: "file_write".to_string(),
            capability: SandboxPermissionCapability::Write,
            path: "/workspace/src/lib.rs".to_string(),
        };

        let prompt = permission_prompt(&denial, None);

        assert!(prompt.starts_with("Linux Bubblewrap blocked"));
        assert!(!prompt.contains("macOS Seatbelt"));
    }

    #[test]
    fn derives_bash_prefix_from_denied_path_position() {
        assert_eq!(
            bash_command_prefix_for_denial("cat \"/Users/me/a.txt\"", "/Users/me/a.txt").as_deref(),
            Some("cat \"")
        );
    }

    #[test]
    fn bash_rule_matches_prefix_and_parent_path() {
        let rules = BashSandboxApprovalRules::default();
        let denial = SandboxPolicyDenial {
            backend_id: "local".to_string(),
            isolation: "macos_seatbelt".to_string(),
            operation: "bash".to_string(),
            capability: SandboxPermissionCapability::Write,
            path: "/Users/me/docs/a.txt".to_string(),
        };

        rules.add("cat \"/Users/me/docs/a.txt\"", &denial);

        let next_denial = SandboxPolicyDenial {
            path: "/Users/me/docs/b.txt".to_string(),
            ..denial
        };
        assert!(rules.matches("cat \"/Users/me/docs/b.txt\"", &next_denial));
        assert!(!rules.matches("sed -n '1p' \"/Users/me/docs/b.txt\"", &next_denial));
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
