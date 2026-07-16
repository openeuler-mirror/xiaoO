use crate::gateway::{ResolvedSessionRuntime, SessionServiceError};
use agent_contracts::backend::{
    capability::{exec::ExecRequest, filesystem::ReadBytesRequest},
    BackendPath, OperationBackend, OperationError, PathKind,
};
use serde::Deserialize;
use skill::{
    loading::{parse_skill_md_content, parse_skill_toml_content},
    FileSkillRegistry,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const REMOTE_MANIFEST_PATH: &str = "/home/user/.xiaoo/bootstrap/manifest.json";
const WORKSPACE_PROMPT_MARKER_BEGIN: &str = "<xiaoo_workspace_prompt>";
const WORKSPACE_PROMPT_MARKER_END: &str = "</xiaoo_workspace_prompt>";
const REPO_MAP_MAX_FILES: usize = 50;
const REPO_MAP_MAX_SIGS_PER_FILE: usize = 8;
const REPO_MAP_MAX_BYTES: usize = 6000;
const REPO_MAP_MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Deserialize)]
struct RemoteManifest {
    version: u32,
    archive_sha256: String,
}

pub(crate) async fn finalize_e2b_runtime(
    resolved: &mut ResolvedSessionRuntime,
    backend: Arc<dyn OperationBackend>,
) -> Result<(), SessionServiceError> {
    if resolved.e2b_finalized {
        return Ok(());
    }
    let Some(binding) = resolved.bootstrap_binding.clone() else {
        resolved.e2b_finalized = true;
        return Ok(());
    };

    verify_remote_manifest(
        backend.as_ref(),
        &binding.content_digest,
        binding.manifest_version,
    )
    .await?;

    if binding.source_workspace.is_some() {
        let workspace_section =
            remote_workspace_section(backend.as_ref(), binding.remote_workspace_root.as_path())
                .await?;
        if !workspace_section.is_empty() {
            if !resolved.descriptor.system_prompt.trim().is_empty() {
                resolved.descriptor.system_prompt.push_str("\n\n");
            }
            resolved
                .descriptor
                .system_prompt
                .push_str(&workspace_section);
        }
    }

    let parsed_skills = load_remote_skills(backend.as_ref(), &binding).await?;
    resolved.skill_registry = Some(Arc::new(FileSkillRegistry::from_skills(parsed_skills)));
    resolved.e2b_finalized = true;
    Ok(())
}

async fn load_remote_skills(
    backend: &dyn OperationBackend,
    binding: &crate::gateway::RuntimeBootstrapBinding,
) -> Result<Vec<skill::Skill>, SessionServiceError> {
    let mut parsed_skills = Vec::new();
    let mut seen_names = HashSet::new();
    for manifest_skill in &binding.skills {
        let manifest_path = manifest_skill
            .remote_dir
            .join(&manifest_skill.manifest_file);
        let content = read_utf8(backend, &manifest_path).await?;
        let parsed = if manifest_skill.manifest_file == "SKILL.toml" {
            let companion_path = manifest_skill.remote_dir.join("SKILL.md");
            let companion = read_optional_utf8(backend, &companion_path).await?;
            parse_skill_toml_content(
                &content,
                companion.as_deref(),
                &manifest_path,
                &manifest_skill.remote_dir,
            )
        } else {
            parse_skill_md_content(&content, &manifest_path, &manifest_skill.remote_dir)
        };
        let mut skill = match parsed {
            Ok(skill) => skill,
            Err(error) => {
                tracing::warn!(
                    path = %manifest_path.display(),
                    %error,
                    "remote E2B skill became invalid; skipping"
                );
                continue;
            }
        };
        if !seen_names.insert(skill.name.clone()) {
            tracing::warn!(name = %skill.name, "duplicate remote E2B skill name; first wins");
            continue;
        }
        skill.location = Some(manifest_skill.remote_dir.clone());
        skill.prompt = format!(
            "{}\n<indicator>skill loaded from {}</indicator>",
            skill.prompt,
            manifest_skill.remote_dir.display()
        );
        parsed_skills.push(skill);
    }
    parsed_skills.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(parsed_skills)
}

async fn verify_remote_manifest(
    backend: &dyn OperationBackend,
    expected_digest: &str,
    expected_version: u32,
) -> Result<(), SessionServiceError> {
    let bytes = backend
        .files()
        .read_bytes(ReadBytesRequest {
            path: BackendPath(REMOTE_MANIFEST_PATH.to_string()),
        })
        .await
        .map_err(|error| SessionServiceError::RuntimeBuild {
            message: format!("failed to read E2B bootstrap manifest: {error}"),
        })?;
    let manifest: RemoteManifest =
        serde_json::from_slice(&bytes).map_err(|error| SessionServiceError::RuntimeBuild {
            message: format!("invalid E2B bootstrap manifest: {error}"),
        })?;
    if manifest.version != expected_version || manifest.archive_sha256 != expected_digest {
        return Err(SessionServiceError::RuntimeConflict {
            message: format!(
                "E2B bootstrap manifest does not match runtime binding (version {}, digest {})",
                manifest.version, manifest.archive_sha256
            ),
        });
    }
    Ok(())
}

async fn remote_workspace_section(
    backend: &dyn OperationBackend,
    workspace_root: &Path,
) -> Result<String, SessionServiceError> {
    let mut sections = Vec::new();
    let agents_path = workspace_root.join("AGENTS.md");
    if let Some(content) = read_optional_utf8(backend, &agents_path).await? {
        let content = content.trim();
        if !content.is_empty() {
            sections.push(format!(
                "{WORKSPACE_PROMPT_MARKER_BEGIN}\n## Workspace Instructions\n\
The following instructions were loaded from the E2B workspace.\n\n### {}\n{}\n\
{WORKSPACE_PROMPT_MARKER_END}",
                agents_path.display(),
                content
            ));
        }
    }
    if let Some(repo_map) = compose_remote_repo_map(backend, workspace_root).await? {
        sections.push(repo_map);
    }
    Ok(sections.join("\n\n"))
}

async fn compose_remote_repo_map(
    backend: &dyn OperationBackend,
    workspace_root: &Path,
) -> Result<Option<String>, SessionServiceError> {
    let output = backend
        .exec()
        .exec(ExecRequest {
            command: "find . -maxdepth 4 -type f -print0".to_string(),
            args: Vec::new(),
            shell: Some("/bin/sh".to_string()),
            cwd: Some(BackendPath(workspace_root.display().to_string())),
            timeout_ms: Some(10_000),
            env: None,
        })
        .await
        .map_err(|error| SessionServiceError::RuntimeBuild {
            message: format!("failed to enumerate E2B workspace for repo map: {error}"),
        })?;
    if output.exit_code != Some(0) {
        return Err(SessionServiceError::RuntimeBuild {
            message: format!(
                "failed to enumerate E2B workspace for repo map: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    let mut relative_files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter_map(|raw| std::str::from_utf8(raw).ok())
        .map(|path| path.trim_start_matches("./"))
        .filter(|path| !path.is_empty())
        .filter(|path| !repo_map_skipped(path) && repo_map_is_source(path))
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    relative_files.sort();
    relative_files.truncate(REPO_MAP_MAX_FILES);
    if relative_files.is_empty() {
        return Ok(None);
    }

    let mut out = String::from(
        "## Repository map\nStatic overview of source files and their top-level definitions \
(not exhaustive; use glob/grep for full detail):\n",
    );
    for relative in relative_files {
        let remote = workspace_root.join(&relative);
        let stat = backend
            .files()
            .stat(&BackendPath(remote.display().to_string()))
            .await
            .map_err(|error| SessionServiceError::RuntimeBuild {
                message: format!(
                    "failed to stat E2B repo-map file {}: {error}",
                    remote.display()
                ),
            })?;
        out.push('\n');
        out.push_str(&relative.display().to_string());
        if stat.kind == Some(PathKind::File)
            && stat.size_bytes.unwrap_or(u64::MAX) <= REPO_MAP_MAX_FILE_BYTES
        {
            if let Ok(content) = read_utf8(backend, &remote).await {
                for signature in repo_map_signatures(&content) {
                    out.push_str("\n  ");
                    out.push_str(&signature);
                }
            }
        }
        if out.len() >= REPO_MAP_MAX_BYTES {
            out.push_str("\n…[repo map truncated]…");
            break;
        }
    }
    Ok(Some(out.trim_end().to_string()))
}

fn repo_map_skipped(path: &str) -> bool {
    path.split('/').any(|segment| {
        segment.starts_with('.')
            || matches!(
                segment,
                "target" | "node_modules" | "__pycache__" | "dist" | "build" | "vendor"
            )
    })
}

fn repo_map_is_source(path: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "rb", "c", "cc", "cpp", "h", "hpp",
        "cs", "php", "kt", "swift", "scala", "sh", "lua", "ex", "exs",
    ];
    path.rsplit('.')
        .next()
        .map(|extension| EXTENSIONS.contains(&extension))
        .unwrap_or(false)
}

fn repo_map_signatures(content: &str) -> Vec<String> {
    const KEYWORDS: &[&str] = &[
        "pub fn ",
        "pub async fn ",
        "async fn ",
        "fn ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "impl ",
        "def ",
        "async def ",
        "class ",
        "func ",
        "function ",
        "export function ",
        "export class ",
        "export const ",
        "export default ",
        "interface ",
        "type ",
    ];
    let mut signatures = Vec::new();
    for line in content.lines().take(3000) {
        let trimmed = line.trim_start();
        if KEYWORDS.iter().any(|keyword| trimmed.starts_with(keyword)) {
            let mut signature = trimmed.chars().take(110).collect::<String>();
            if let Some(index) = signature.find(['{', ';', '=']) {
                signature.truncate(index);
            }
            let signature = signature.trim_end().to_string();
            if !signature.is_empty() {
                signatures.push(signature);
            }
            if signatures.len() >= REPO_MAP_MAX_SIGS_PER_FILE {
                break;
            }
        }
    }
    signatures
}

async fn read_optional_utf8(
    backend: &dyn OperationBackend,
    path: &Path,
) -> Result<Option<String>, SessionServiceError> {
    match backend
        .files()
        .read_bytes(ReadBytesRequest {
            path: BackendPath(path.display().to_string()),
        })
        .await
    {
        Ok(bytes) => {
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| SessionServiceError::RuntimeBuild {
                    message: format!("E2B file is not UTF-8 {}: {error}", path.display()),
                })
        }
        Err(OperationError::NotFound { .. }) => Ok(None),
        Err(error) => Err(SessionServiceError::RuntimeBuild {
            message: format!("failed to read E2B file {}: {error}", path.display()),
        }),
    }
}

async fn read_utf8(
    backend: &dyn OperationBackend,
    path: &Path,
) -> Result<String, SessionServiceError> {
    read_optional_utf8(backend, path)
        .await?
        .ok_or_else(|| SessionServiceError::RuntimeBuild {
            message: format!("required E2B file is missing: {}", path.display()),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_contracts::backend::capability::{
        export::{ExportFileRequest, OperationExport},
        filesystem::{OperationFileSystem, TempPathRequest, WriteBytesOutcome, WriteBytesRequest},
        path::{OperationPathResolver, ResolvePathRequest},
        search::{GlobRequest, GrepRequest, GrepResult, OperationSearch},
        OperationExec,
    };
    use agent_contracts::backend::{
        OperationBackendCapabilities, PathStat, SharedExportedFileHandle,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::RwLock;

    struct FakeBackend {
        root: BackendPath,
        files: RwLock<HashMap<String, Vec<u8>>>,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                root: BackendPath("/home/user/workspace".to_string()),
                files: RwLock::new(HashMap::new()),
            }
        }

        fn put(&self, path: &str, content: impl Into<Vec<u8>>) {
            self.files
                .write()
                .unwrap()
                .insert(path.to_string(), content.into());
        }
    }

    fn unsupported() -> OperationError {
        OperationError::Unsupported {
            message: "not used by test".to_string(),
        }
    }

    #[async_trait]
    impl OperationFileSystem for FakeBackend {
        async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
            let files = self.files.read().unwrap();
            let content = files.get(&path.0);
            Ok(PathStat {
                exists: content.is_some(),
                kind: content.map(|_| PathKind::File),
                size_bytes: content.map(|content| content.len() as u64),
                modified_at: None,
            })
        }

        async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
            self.files
                .read()
                .unwrap()
                .get(&request.path.0)
                .cloned()
                .ok_or(OperationError::NotFound {
                    path: request.path.0,
                })
        }

        async fn write_bytes(
            &self,
            _request: WriteBytesRequest,
        ) -> Result<WriteBytesOutcome, OperationError> {
            Err(unsupported())
        }

        async fn create_dir_all(&self, _path: &BackendPath) -> Result<(), OperationError> {
            Err(unsupported())
        }

        async fn temp_path(
            &self,
            _request: TempPathRequest,
        ) -> Result<BackendPath, OperationError> {
            Err(unsupported())
        }
    }

    #[async_trait]
    impl OperationExec for FakeBackend {
        async fn exec(
            &self,
            _request: ExecRequest,
        ) -> Result<agent_contracts::backend::capability::exec::ExecResult, OperationError>
        {
            let prefix = format!("{}/", self.root.0);
            let mut paths = self
                .files
                .read()
                .unwrap()
                .keys()
                .filter_map(|path| path.strip_prefix(&prefix))
                .map(|path| format!("./{path}"))
                .collect::<Vec<_>>();
            paths.sort();
            let mut stdout = Vec::new();
            for path in paths {
                stdout.extend_from_slice(path.as_bytes());
                stdout.push(0);
            }
            Ok(agent_contracts::backend::capability::exec::ExecResult {
                stdout,
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            })
        }
    }

    #[async_trait]
    impl OperationPathResolver for FakeBackend {
        fn workspace_root(&self) -> &BackendPath {
            &self.root
        }

        fn home_dir(&self) -> Option<&BackendPath> {
            None
        }

        async fn resolve_path(
            &self,
            request: ResolvePathRequest,
        ) -> Result<BackendPath, OperationError> {
            Ok(BackendPath(request.raw_path))
        }
    }

    #[async_trait]
    impl OperationSearch for FakeBackend {
        async fn glob(&self, _request: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
            Err(unsupported())
        }

        async fn grep(&self, _request: GrepRequest) -> Result<GrepResult, OperationError> {
            Err(unsupported())
        }
    }

    #[async_trait]
    impl OperationExport for FakeBackend {
        async fn export_file(
            &self,
            _request: ExportFileRequest,
        ) -> Result<SharedExportedFileHandle, OperationError> {
            Err(unsupported())
        }
    }

    #[async_trait]
    impl OperationBackend for FakeBackend {
        fn backend_id(&self) -> &str {
            "fake-e2b"
        }

        fn capabilities(&self) -> OperationBackendCapabilities {
            OperationBackendCapabilities {
                supports_atomic_write: true,
                supports_grep: true,
                supports_export_file: false,
                supports_lsp: false,
            }
        }

        fn paths(&self) -> &dyn OperationPathResolver {
            self
        }

        fn files(&self) -> &dyn OperationFileSystem {
            self
        }

        fn search(&self) -> &dyn OperationSearch {
            self
        }

        fn exec(&self) -> &dyn OperationExec {
            self
        }

        fn export(&self) -> &dyn OperationExport {
            self
        }

        async fn shutdown(&self) -> Result<(), OperationError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn workspace_context_uses_remote_files_and_reflects_changes() {
        let backend = FakeBackend::new();
        backend.put(
            "/home/user/workspace/AGENTS.md",
            "remote instructions".as_bytes(),
        );
        backend.put(
            "/home/user/workspace/src/lib.rs",
            "pub fn first() {}".as_bytes(),
        );

        let first = remote_workspace_section(&backend, Path::new("/home/user/workspace"))
            .await
            .unwrap();
        assert!(first.contains("/home/user/workspace/AGENTS.md"));
        assert!(first.contains("remote instructions"));
        assert!(first.contains("src/lib.rs"));
        assert!(first.contains("pub fn first()"));

        backend.put(
            "/home/user/workspace/src/new.py",
            "def added():\n    pass\n".as_bytes(),
        );
        let second = remote_workspace_section(&backend, Path::new("/home/user/workspace"))
            .await
            .unwrap();
        assert!(second.contains("src/new.py"));
        assert!(second.contains("def added():"));
    }

    #[tokio::test]
    async fn remote_skill_registry_data_exposes_only_remote_paths() {
        let backend = FakeBackend::new();
        let remote_dir = PathBuf::from("/home/user/.xiaoo/skills/0/skill-00000");
        backend.put(
            "/home/user/.xiaoo/skills/0/skill-00000/SKILL.md",
            "---\nname: remote-skill\ndescription: remote\n---\nUse the asset.".as_bytes(),
        );
        let binding = crate::gateway::RuntimeBootstrapBinding {
            source_workspace: None,
            source_skill_roots: vec![PathBuf::from("/host/skills")],
            content_digest: "digest".to_string(),
            remote_workspace_root: PathBuf::from("/home/user/workspace"),
            remote_skill_roots: vec![PathBuf::from("/home/user/.xiaoo/skills/0")],
            skills: vec![crate::gateway::RuntimeBootstrapSkill {
                name: "remote-skill".to_string(),
                remote_dir: remote_dir.clone(),
                manifest_file: "SKILL.md".to_string(),
            }],
            manifest_version: crate::gateway::E2B_BOOTSTRAP_MANIFEST_VERSION,
        };

        let skills = load_remote_skills(&backend, &binding).await.unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].location.as_ref(), Some(&remote_dir));
        assert!(skills[0].prompt.contains(&remote_dir.display().to_string()));
        assert!(!skills[0].prompt.contains("/host/skills"));
    }

    #[tokio::test]
    async fn remote_manifest_digest_mismatch_is_a_conflict() {
        let backend = FakeBackend::new();
        backend.put(
            REMOTE_MANIFEST_PATH,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "archive_sha256": "different"
            }))
            .unwrap(),
        );

        let error = verify_remote_manifest(&backend, "expected", 1)
            .await
            .expect_err("digest mismatch");
        assert!(matches!(error, SessionServiceError::RuntimeConflict { .. }));
    }
}
