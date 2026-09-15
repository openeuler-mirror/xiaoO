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

    let error = build_e2b_bootstrap_archive(Some(workspace), Vec::new(), &SkillsConfig::default())
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

    let error = build_e2b_bootstrap_archive(Some(workspace), Vec::new(), &SkillsConfig::default())
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
