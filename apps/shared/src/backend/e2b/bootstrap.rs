use crate::gateway::{
    RuntimeBootstrapBinding, RuntimeBootstrapSkill, E2B_BOOTSTRAP_MANIFEST_VERSION,
    E2B_REMOTE_SKILLS_ROOT, E2B_REMOTE_WORKSPACE_ROOT,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use skill::{loading::load_skills, SkillsConfig};
use std::fs::{File, Metadata};
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

pub(crate) const E2B_BOOTSTRAP_MAX_ENTRIES: u64 = 100_000;
pub(crate) const E2B_BOOTSTRAP_MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const E2B_BOOTSTRAP_MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// A disk-backed, immutable bootstrap payload. The file is removed when the
/// last runtime/build reference is dropped.
pub struct E2bBootstrapArchive {
    path: PathBuf,
    sha256: String,
    size_bytes: u64,
    binding: RuntimeBootstrapBinding,
}

impl std::fmt::Debug for E2bBootstrapArchive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2bBootstrapArchive")
            .field("path", &self.path)
            .field("sha256", &self.sha256)
            .field("size_bytes", &self.size_bytes)
            .finish_non_exhaustive()
    }
}

impl E2bBootstrapArchive {
    // `path()` is read only inside the crate (e2b provider + bootstrap
    // tests); app code consumes `binding()` only, so keep the filesystem
    // path accessor crate-private.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn sha256(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn binding(&self) -> &RuntimeBootstrapBinding {
        &self.binding
    }

    pub(crate) fn manifest_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        #[derive(Serialize)]
        struct Manifest<'a> {
            version: u32,
            archive_sha256: &'a str,
            binding: &'a RuntimeBootstrapBinding,
        }
        serde_json::to_vec(&Manifest {
            version: E2B_BOOTSTRAP_MANIFEST_VERSION,
            archive_sha256: &self.sha256,
            binding: &self.binding,
        })
    }
}

impl Drop for E2bBootstrapArchive {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(path = %self.path.display(), %error, "failed to remove E2B bootstrap archive");
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum E2bBootstrapBuildError {
    #[error("invalid bootstrap path: {message}")]
    InvalidPath { message: String },
    #[error("bootstrap source changed while being archived: {path}")]
    SourceChanged { path: PathBuf },
    #[error("bootstrap capacity exceeded: {message}")]
    CapacityExceeded { message: String },
    #[error("failed to build bootstrap archive: {message}")]
    BuildFailed { message: String },
}

pub fn canonicalize_bootstrap_dir(path: &Path) -> Result<PathBuf, E2bBootstrapBuildError> {
    if !path.is_absolute() {
        return Err(E2bBootstrapBuildError::InvalidPath {
            message: format!("path must be absolute: {}", path.display()),
        });
    }
    let canonical =
        std::fs::canonicalize(path).map_err(|error| E2bBootstrapBuildError::InvalidPath {
            message: format!("cannot access {}: {error}", path.display()),
        })?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|error| E2bBootstrapBuildError::InvalidPath {
            message: format!("cannot read {}: {error}", canonical.display()),
        })?;
    if !metadata.is_dir() {
        return Err(E2bBootstrapBuildError::InvalidPath {
            message: format!("path is not a directory: {}", canonical.display()),
        });
    }
    std::fs::read_dir(&canonical).map_err(|error| E2bBootstrapBuildError::InvalidPath {
        message: format!("directory is not readable {}: {error}", canonical.display()),
    })?;
    Ok(canonical)
}

/// Build an uncompressed tar containing an empty-or-complete workspace plus
/// only the skills that won deterministic parsing/auditing/deduplication.
pub fn build_e2b_bootstrap_archive(
    workspace: Option<PathBuf>,
    skill_roots: Vec<PathBuf>,
    skill_policy: &SkillsConfig,
) -> Result<E2bBootstrapArchive, E2bBootstrapBuildError> {
    let mut config = skill_policy.clone();
    config.skills_dirs = skill_roots.clone();
    let enabled_skills = load_skills(&config);

    let named = tempfile::Builder::new()
        .prefix("xiaoo-e2b-bootstrap-")
        .suffix(".tar")
        .tempfile()
        .map_err(build_io("create temporary archive"))?;
    let (mut file, archive_path) =
        named
            .keep()
            .map_err(|error| E2bBootstrapBuildError::BuildFailed {
                message: format!("persist temporary archive: {}", error.error),
            })?;
    let mut cleanup = ArchivePathGuard(Some(archive_path.clone()));

    let mut limits = ArchiveLimits::default();
    let mut binding_skills = Vec::new();
    {
        let mut tar = tar::Builder::new(&mut file);
        tar.mode(tar::HeaderMode::Deterministic);
        if let Some(source) = workspace.as_deref() {
            let metadata = std::fs::metadata(source).map_err(build_io("stat workspace root"))?;
            append_dir(&mut tar, Path::new("workspace"), &metadata)?;
        } else {
            append_synthetic_dir(&mut tar, Path::new("workspace"))?;
        }
        append_synthetic_dir(&mut tar, Path::new("skills"))?;

        if let Some(source) = workspace.as_deref() {
            limits.add_entry(source)?;
            append_tree(
                &mut tar,
                source,
                Path::new("workspace"),
                Path::new(E2B_REMOTE_WORKSPACE_ROOT),
                &mut limits,
            )?;
        }

        for (index, root) in skill_roots.iter().enumerate() {
            limits.add_entry(root)?;
            let metadata = std::fs::metadata(root).map_err(build_io("stat skill root"))?;
            append_dir(
                &mut tar,
                &PathBuf::from(format!("skills/{index}")),
                &metadata,
            )?;
        }

        for (ordinal, skill) in enabled_skills.iter().enumerate() {
            let source =
                skill
                    .location
                    .as_deref()
                    .ok_or_else(|| E2bBootstrapBuildError::BuildFailed {
                        message: format!("loaded skill '{}' has no source directory", skill.name),
                    })?;
            let source = std::fs::canonicalize(source).map_err(build_io("canonicalize skill"))?;
            let root_index = skill_roots
                .iter()
                .position(|root| source.starts_with(root))
                .ok_or_else(|| E2bBootstrapBuildError::InvalidPath {
                    message: format!(
                        "skill '{}' resolved outside all requested roots: {}",
                        skill.name,
                        source.display()
                    ),
                })?;
            let leaf = format!("skill-{ordinal:05}");
            let archive_dir = PathBuf::from(format!("skills/{root_index}/{leaf}"));
            let remote_dir = PathBuf::from(E2B_REMOTE_SKILLS_ROOT)
                .join(root_index.to_string())
                .join(&leaf);
            let metadata = std::fs::metadata(&source).map_err(build_io("stat skill directory"))?;
            limits.add_entry(&source)?;
            append_dir(&mut tar, &archive_dir, &metadata)?;
            append_tree(&mut tar, &source, &archive_dir, &remote_dir, &mut limits)?;
            let manifest_file = if source.join("SKILL.toml").is_file() {
                "SKILL.toml"
            } else {
                "SKILL.md"
            };
            binding_skills.push(RuntimeBootstrapSkill {
                name: skill.name.clone(),
                remote_dir,
                manifest_file: manifest_file.to_string(),
            });
        }
        tar.finish().map_err(build_io("finish archive"))?;
    }
    file.sync_all().map_err(build_io("sync archive"))?;
    drop(file);

    let (sha256, size_bytes) = hash_file(&archive_path)?;
    #[cfg(unix)]
    std::fs::set_permissions(&archive_path, std::fs::Permissions::from_mode(0o400))
        .map_err(build_io("seal completed archive"))?;
    let binding = RuntimeBootstrapBinding {
        source_workspace: workspace,
        source_skill_roots: skill_roots,
        content_digest: sha256.clone(),
        remote_workspace_root: PathBuf::from(E2B_REMOTE_WORKSPACE_ROOT),
        remote_skill_roots: config
            .skills_dirs
            .iter()
            .enumerate()
            .map(|(index, _)| PathBuf::from(E2B_REMOTE_SKILLS_ROOT).join(index.to_string()))
            .collect(),
        skills: binding_skills,
        manifest_version: E2B_BOOTSTRAP_MANIFEST_VERSION,
    };

    cleanup.0.take();
    Ok(E2bBootstrapArchive {
        path: archive_path,
        sha256,
        size_bytes,
        binding,
    })
}

struct ArchivePathGuard(Option<PathBuf>);

impl Drop for ArchivePathGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            std::fs::remove_file(path).ok();
        }
    }
}

#[derive(Default)]
struct ArchiveLimits {
    entries: u64,
    total_bytes: u64,
}

impl ArchiveLimits {
    fn add_entry(&mut self, path: &Path) -> Result<(), E2bBootstrapBuildError> {
        self.entries = self.entries.saturating_add(1);
        if self.entries > E2B_BOOTSTRAP_MAX_ENTRIES {
            return Err(E2bBootstrapBuildError::CapacityExceeded {
                message: format!(
                    "more than {E2B_BOOTSTRAP_MAX_ENTRIES} entries (at {})",
                    path.display()
                ),
            });
        }
        Ok(())
    }

    fn add_file(&mut self, path: &Path, size: u64) -> Result<(), E2bBootstrapBuildError> {
        self.add_entry(path)?;
        if size > E2B_BOOTSTRAP_MAX_FILE_BYTES {
            return Err(E2bBootstrapBuildError::CapacityExceeded {
                message: format!(
                    "file {} is {size} bytes; per-file limit is {E2B_BOOTSTRAP_MAX_FILE_BYTES}",
                    path.display()
                ),
            });
        }
        self.total_bytes = self.total_bytes.saturating_add(size);
        if self.total_bytes > E2B_BOOTSTRAP_MAX_TOTAL_BYTES {
            return Err(E2bBootstrapBuildError::CapacityExceeded {
                message: format!(
                    "regular-file content exceeds {E2B_BOOTSTRAP_MAX_TOTAL_BYTES} bytes"
                ),
            });
        }
        Ok(())
    }
}

fn append_tree(
    tar: &mut tar::Builder<&mut File>,
    source_root: &Path,
    archive_root: &Path,
    remote_root: &Path,
    limits: &mut ArchiveLimits,
) -> Result<(), E2bBootstrapBuildError> {
    let before = std::fs::metadata(source_root).map_err(build_io("stat source directory"))?;
    append_children(
        tar,
        source_root,
        source_root,
        archive_root,
        remote_root,
        limits,
    )?;
    let after = std::fs::metadata(source_root).map_err(build_io("restat source directory"))?;
    if !same_metadata(&before, &after) {
        return Err(E2bBootstrapBuildError::SourceChanged {
            path: source_root.to_path_buf(),
        });
    }
    Ok(())
}

fn append_children(
    tar: &mut tar::Builder<&mut File>,
    source_root: &Path,
    source_dir: &Path,
    archive_dir: &Path,
    remote_root: &Path,
    limits: &mut ArchiveLimits,
) -> Result<(), E2bBootstrapBuildError> {
    let before = std::fs::symlink_metadata(source_dir).map_err(build_io("stat directory"))?;
    let mut entries = std::fs::read_dir(source_dir)
        .map_err(build_io("read source directory"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(build_io("read source entry"))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source = entry.path();
        let metadata = std::fs::symlink_metadata(&source).map_err(build_io("stat source entry"))?;
        let destination = archive_dir.join(entry.file_name());
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            limits.add_entry(&source)?;
            append_dir(tar, &destination, &metadata)?;
            append_children(tar, source_root, &source, &destination, remote_root, limits)?;
        } else if file_type.is_file() {
            limits.add_file(&source, metadata.len())?;
            append_file(tar, &source, &destination, &metadata)?;
        } else if file_type.is_symlink() {
            limits.add_entry(&source)?;
            append_symlink(
                tar,
                source_root,
                remote_root,
                &source,
                &destination,
                &metadata,
            )?;
        } else {
            #[cfg(unix)]
            let kind = if file_type.is_socket() {
                "socket"
            } else if file_type.is_fifo() {
                "FIFO"
            } else if file_type.is_block_device() {
                "block device"
            } else if file_type.is_char_device() {
                "character device"
            } else {
                "special file"
            };
            #[cfg(not(unix))]
            let kind = "special file";
            return Err(E2bBootstrapBuildError::InvalidPath {
                message: format!("unsupported {kind}: {}", source.display()),
            });
        }
    }
    let after = std::fs::symlink_metadata(source_dir).map_err(build_io("restat directory"))?;
    if !same_metadata(&before, &after) {
        return Err(E2bBootstrapBuildError::SourceChanged {
            path: source_dir.to_path_buf(),
        });
    }
    Ok(())
}

fn append_synthetic_dir(
    tar: &mut tar::Builder<&mut File>,
    path: &Path,
) -> Result<(), E2bBootstrapBuildError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    header.set_mode(0o755);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    tar.append_data(&mut header, path, io::empty())
        .map_err(build_io("append directory"))
}

fn append_dir(
    tar: &mut tar::Builder<&mut File>,
    destination: &Path,
    metadata: &Metadata,
) -> Result<(), E2bBootstrapBuildError> {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Directory);
    header.set_size(0);
    header.set_mode(mode(metadata));
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    tar.append_data(&mut header, destination, io::empty())
        .map_err(build_io("append directory"))
}

fn append_file(
    tar: &mut tar::Builder<&mut File>,
    source: &Path,
    destination: &Path,
    before: &Metadata,
) -> Result<(), E2bBootstrapBuildError> {
    let mut input = File::open(source).map_err(build_io("open source file"))?;
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(before.len());
    header.set_mode(mode(before));
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    let append_result = tar.append_data(&mut header, destination, &mut input);
    let after = input.metadata().map_err(build_io("restat source file"))?;
    if let Err(error) = append_result {
        if !same_metadata(before, &after) {
            return Err(E2bBootstrapBuildError::SourceChanged {
                path: source.to_path_buf(),
            });
        }
        return Err(E2bBootstrapBuildError::BuildFailed {
            message: format!("append source file: {error}"),
        });
    }
    if !same_metadata(before, &after) {
        return Err(E2bBootstrapBuildError::SourceChanged {
            path: source.to_path_buf(),
        });
    }
    Ok(())
}

fn append_symlink(
    tar: &mut tar::Builder<&mut File>,
    source_root: &Path,
    remote_root: &Path,
    source: &Path,
    destination: &Path,
    metadata: &Metadata,
) -> Result<(), E2bBootstrapBuildError> {
    let raw_target = std::fs::read_link(source).map_err(build_io("read symbolic link"))?;
    let resolved = if raw_target.is_absolute() {
        raw_target.clone()
    } else {
        source.parent().unwrap_or(source_root).join(&raw_target)
    };
    let resolved =
        std::fs::canonicalize(&resolved).map_err(|error| E2bBootstrapBuildError::InvalidPath {
            message: format!("invalid symbolic link {}: {error}", source.display()),
        })?;
    if !resolved.starts_with(source_root) {
        return Err(E2bBootstrapBuildError::InvalidPath {
            message: format!(
                "symbolic link escapes input root: {} -> {}",
                source.display(),
                raw_target.display()
            ),
        });
    }
    let target = if raw_target.is_absolute() {
        remote_root.join(resolved.strip_prefix(source_root).unwrap_or(Path::new("")))
    } else {
        raw_target.clone()
    };
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(mode(metadata));
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header
        .set_link_name(&target)
        .map_err(build_io("set symbolic link target"))?;
    header.set_cksum();
    tar.append_data(&mut header, destination, io::empty())
        .map_err(build_io("append symbolic link"))?;
    let after = std::fs::symlink_metadata(source).map_err(build_io("restat symbolic link"))?;
    let current = std::fs::read_link(source).map_err(build_io("reread symbolic link"))?;
    if !same_metadata(metadata, &after) || current != raw_target {
        return Err(E2bBootstrapBuildError::SourceChanged {
            path: source.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(unix)]
fn mode(metadata: &Metadata) -> u32 {
    metadata.mode() & 0o7777
}

#[cfg(not(unix))]
fn mode(_metadata: &Metadata) -> u32 {
    0o644
}

#[cfg(unix)]
fn same_metadata(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.mode() == right.mode()
        && left.size() == right.size()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn same_metadata(left: &Metadata, right: &Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.permissions().readonly() == right.permissions().readonly()
}

fn hash_file(path: &Path) -> Result<(String, u64), E2bBootstrapBuildError> {
    let file = File::open(path).map_err(build_io("open completed archive"))?;
    let size = file
        .metadata()
        .map_err(build_io("stat completed archive"))?
        .len();
    let mut reader = BufReader::new(file);
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(build_io("hash completed archive"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

fn build_io(operation: &'static str) -> impl FnOnce(io::Error) -> E2bBootstrapBuildError {
    move |error| E2bBootstrapBuildError::BuildFailed {
        message: format!("{operation}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn extract(archive: &E2bBootstrapArchive, destination: &Path) {
        let file = File::open(archive.path()).expect("open archive");
        let mut archive = tar::Archive::new(file);
        archive.unpack(destination).expect("extract archive");
    }

    #[test]
    fn empty_inputs_produce_empty_replacement_roots() {
        let archive = build_e2b_bootstrap_archive(None, Vec::new(), &SkillsConfig::default())
            .expect("empty bootstrap");
        let extracted = tempfile::tempdir().expect("extract dir");
        extract(&archive, extracted.path());

        assert!(extracted.path().join("workspace").is_dir());
        assert!(extracted.path().join("skills").is_dir());
        assert!(archive.binding().source_workspace.is_none());
        assert!(archive.binding().source_skill_roots.is_empty());
        assert_eq!(archive.binding().skills, Vec::new());
    }

    #[cfg(unix)]
    #[test]
    fn snapshots_hidden_git_empty_modes_symlinks_and_winning_skills() {
        let host = tempfile::tempdir().expect("host");
        let workspace = host.path().join("workspace");
        fs::create_dir_all(workspace.join(".git")).expect("git dir");
        fs::create_dir(workspace.join("empty")).expect("empty dir");
        fs::write(workspace.join(".hidden"), "hidden").expect("hidden file");
        fs::write(workspace.join(".git/config"), "git config").expect("git config");
        fs::write(workspace.join("run.sh"), "#!/bin/sh\n").expect("script");
        fs::set_permissions(workspace.join("run.sh"), fs::Permissions::from_mode(0o755))
            .expect("chmod");
        symlink("run.sh", workspace.join("run-link")).expect("internal symlink");
        symlink(
            workspace.join("run.sh"),
            workspace.join("absolute-run-link"),
        )
        .expect("absolute internal symlink");

        let first_root = host.path().join("skills-first");
        let second_root = host.path().join("skills-second");
        fs::create_dir_all(first_root.join("alpha/assets")).expect("first skill");
        fs::create_dir_all(first_root.join("alpha/scripts")).expect("scripts");
        fs::write(
            first_root.join("alpha/SKILL.md"),
            "---\nname: duplicate\ndescription: first\n---\nfirst prompt",
        )
        .expect("first manifest");
        fs::write(first_root.join("alpha/assets/data.txt"), "asset").expect("asset");
        fs::write(first_root.join("alpha/scripts/run.sh"), "echo first").expect("skill script");
        fs::create_dir_all(second_root.join("beta")).expect("second duplicate");
        fs::write(
            second_root.join("beta/SKILL.md"),
            "---\nname: duplicate\ndescription: second\n---\nsecond prompt",
        )
        .expect("second manifest");
        fs::create_dir_all(second_root.join("gamma")).expect("unique skill");
        fs::write(
            second_root.join("gamma/SKILL.md"),
            "---\nname: unique\ndescription: unique\n---\nunique prompt",
        )
        .expect("unique manifest");

        let workspace = canonicalize_bootstrap_dir(&workspace).expect("workspace canonical");
        let first_root = canonicalize_bootstrap_dir(&first_root).expect("first canonical");
        let second_root = canonicalize_bootstrap_dir(&second_root).expect("second canonical");
        let archive = build_e2b_bootstrap_archive(
            Some(workspace),
            vec![first_root, second_root],
            &SkillsConfig::default(),
        )
        .expect("bootstrap");
        let extracted = tempfile::tempdir().expect("extract dir");
        extract(&archive, extracted.path());

        assert_eq!(
            fs::read_to_string(extracted.path().join("workspace/.git/config")).unwrap(),
            "git config"
        );
        assert!(extracted.path().join("workspace/empty").is_dir());
        assert_eq!(
            fs::metadata(extracted.path().join("workspace/run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_link(extracted.path().join("workspace/run-link")).unwrap(),
            PathBuf::from("run.sh")
        );
        assert_eq!(
            fs::read_link(extracted.path().join("workspace/absolute-run-link")).unwrap(),
            PathBuf::from("/home/user/workspace/run.sh")
        );

        let skills = &archive.binding().skills;
        assert_eq!(skills.len(), 2);
        let duplicate = skills
            .iter()
            .find(|skill| skill.name == "duplicate")
            .unwrap();
        assert!(duplicate
            .remote_dir
            .starts_with("/home/user/.xiaoo/skills/0"));
        let archive_duplicate = PathBuf::from("skills").join(
            duplicate
                .remote_dir
                .strip_prefix(E2B_REMOTE_SKILLS_ROOT)
                .unwrap(),
        );
        assert_eq!(
            fs::read_to_string(
                extracted
                    .path()
                    .join(archive_duplicate)
                    .join("assets/data.txt")
            )
            .unwrap(),
            "asset"
        );
        assert!(skills
            .iter()
            .find(|skill| skill.name == "unique")
            .unwrap()
            .remote_dir
            .starts_with("/home/user/.xiaoo/skills/1"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_that_escapes_workspace() {
        let host = tempfile::tempdir().expect("host");
        let workspace = host.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        fs::write(host.path().join("secret"), "secret").expect("secret");
        symlink("../secret", workspace.join("escape")).expect("symlink");
        let workspace = canonicalize_bootstrap_dir(&workspace).expect("canonical");

        let error =
            build_e2b_bootstrap_archive(Some(workspace), Vec::new(), &SkillsConfig::default())
                .expect_err("escaping link must fail");
        assert!(matches!(error, E2bBootstrapBuildError::InvalidPath { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifo_special_file() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let host = tempfile::tempdir().expect("host");
        let workspace = host.path().join("workspace");
        fs::create_dir(&workspace).expect("workspace");
        let fifo = workspace.join("pipe");
        let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let workspace = canonicalize_bootstrap_dir(&workspace).expect("canonical");

        let error =
            build_e2b_bootstrap_archive(Some(workspace), Vec::new(), &SkillsConfig::default())
                .expect_err("FIFO must fail");
        assert!(matches!(error, E2bBootstrapBuildError::InvalidPath { .. }));
    }

    #[test]
    fn enforces_all_capacity_counters() {
        let mut limits = ArchiveLimits {
            entries: E2B_BOOTSTRAP_MAX_ENTRIES,
            total_bytes: 0,
        };
        assert!(matches!(
            limits.add_entry(Path::new("overflow")),
            Err(E2bBootstrapBuildError::CapacityExceeded { .. })
        ));

        let mut limits = ArchiveLimits::default();
        assert!(matches!(
            limits.add_file(Path::new("large"), E2B_BOOTSTRAP_MAX_FILE_BYTES + 1),
            Err(E2bBootstrapBuildError::CapacityExceeded { .. })
        ));

        let mut limits = ArchiveLimits {
            entries: 0,
            total_bytes: E2B_BOOTSTRAP_MAX_TOTAL_BYTES,
        };
        assert!(matches!(
            limits.add_file(Path::new("total"), 1),
            Err(E2bBootstrapBuildError::CapacityExceeded { .. })
        ));
    }

    #[test]
    fn distinct_host_workspaces_share_only_the_remote_mount_point() {
        let host = tempfile::tempdir().expect("host");
        let cz = host.path().join("cz");
        let xxy = host.path().join("xxy");
        fs::create_dir(&cz).unwrap();
        fs::create_dir(&xxy).unwrap();
        fs::write(cz.join("owner.txt"), "cz").unwrap();
        fs::write(xxy.join("owner.txt"), "xxy").unwrap();
        let cz = canonicalize_bootstrap_dir(&cz).unwrap();
        let xxy = canonicalize_bootstrap_dir(&xxy).unwrap();

        let cz_archive =
            build_e2b_bootstrap_archive(Some(cz.clone()), Vec::new(), &SkillsConfig::default())
                .unwrap();
        let xxy_archive =
            build_e2b_bootstrap_archive(Some(xxy.clone()), Vec::new(), &SkillsConfig::default())
                .unwrap();

        assert_eq!(cz_archive.binding().source_workspace.as_ref(), Some(&cz));
        assert_eq!(xxy_archive.binding().source_workspace.as_ref(), Some(&xxy));
        assert_eq!(
            cz_archive.binding().remote_workspace_root,
            xxy_archive.binding().remote_workspace_root
        );
        assert_ne!(cz_archive.sha256(), xxy_archive.sha256());
    }
}
