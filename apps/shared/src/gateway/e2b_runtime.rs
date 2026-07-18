use crate::gateway::{
    backend_workspace_context::{
        compose_backend_workspace_section, read_backend_utf8, read_optional_backend_utf8,
    },
    ResolvedSessionRuntime, SessionServiceError,
};
use agent_contracts::backend::{
    capability::filesystem::ReadBytesRequest, BackendPath, OperationBackend,
};
use serde::Deserialize;
use skill::{
    loading::{parse_skill_md_content, parse_skill_toml_content},
    FileSkillRegistry,
};
use std::collections::HashSet;
use std::sync::Arc;

const REMOTE_MANIFEST_PATH: &str = "/home/user/.xiaoo/bootstrap/manifest.json";

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
        let workspace_section = compose_backend_workspace_section(
            backend.as_ref(),
            binding.remote_workspace_root.as_path(),
        )
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
        let content = read_backend_utf8(backend, &manifest_path).await?;
        let parsed = if manifest_skill.manifest_file == "SKILL.toml" {
            let companion_path = manifest_skill.remote_dir.join("SKILL.md");
            let companion = read_optional_backend_utf8(backend, &companion_path).await?;
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
