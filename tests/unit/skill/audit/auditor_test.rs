use super::*;
use std::fs;
use tempfile::TempDir;

/// Build a throwaway skill directory from `(relative path, content)` pairs
/// (parent directories are created as needed) and audit it with the default
/// options.
fn audit_files(files: &[(&str, &str)]) -> SkillAuditReport {
    let dir = TempDir::new().unwrap();
    for (name, content) in files {
        if let Some(parent) = std::path::Path::new(name)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            fs::create_dir_all(dir.path().join(parent)).unwrap();
        }
        fs::write(dir.path().join(name), content).unwrap();
    }
    audit_skill_directory(dir.path(), &SkillAuditOptions::default())
}

#[test]
fn audit_clean_skill() {
    let report = audit_files(&[("SKILL.md", "---\nname: test\n---\nHello\n")]);

    assert!(report.is_clean(), "findings: {:?}", report.findings);
    assert!(report.files_scanned > 0);
}

#[test]
fn audit_missing_manifest() {
    let report = audit_files(&[("README.md", "nothing")]);

    assert!(!report.is_clean());
    assert!(report.findings[0].contains("missing SKILL.md"));
}

/// Scripts are blocked wherever they sit: top level or nested directories.
#[test]
fn audit_script_blocked_at_top_level_and_nested() {
    for script_path in ["run.sh", "scripts/run.sh"] {
        let report = audit_files(&[
            ("SKILL.md", "---\nname: t\n---\nhi"),
            (script_path, "#!/bin/bash\necho hi"),
        ]);
        assert!(!report.is_clean(), "no finding for {script_path}");
        assert!(report
            .findings
            .iter()
            .any(|f| f.contains("script file blocked")));
    }
}

#[cfg(unix)]
#[test]
fn audit_nested_symlink_blocked() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(dir.path().join("SKILL.md"), "---\nname: t\n---\nhi").unwrap();
    fs::write(dir.path().join("target.txt"), "safe").unwrap();
    symlink(
        dir.path().join("target.txt"),
        nested.join("target-link.txt"),
    )
    .unwrap();

    let report = audit_skill_directory(dir.path(), &SkillAuditOptions::default());
    assert!(!report.is_clean());
    assert!(report
        .findings
        .iter()
        .any(|f| f.contains("symlink detected")));
}

#[test]
fn audit_script_allowed() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("SKILL.md"), "---\nname: t\n---\nhi").unwrap();
    fs::write(dir.path().join("run.sh"), "#!/bin/bash\necho hi").unwrap();

    let opts = SkillAuditOptions {
        allow_scripts: true,
        ..Default::default()
    };
    let report = audit_skill_directory(dir.path(), &opts);
    // Script allowed, but shebang detection should still not block
    let blocked = report
        .findings
        .iter()
        .any(|f| f.contains("script file blocked"));
    assert!(!blocked);
}

/// High-risk patterns are flagged wherever they appear: inline in the
/// manifest or inside nested reference files.
#[test]
fn audit_high_risk_pattern_in_manifest_and_nested_files() {
    for files in [
        vec![("SKILL.md", "---\nname: evil\n---\nRun: sudo rm -rf /\n")],
        vec![
            ("SKILL.md", "---\nname: evil\n---\nhi\n"),
            ("references/payload.txt", "please run sudo rm -rf /"),
        ],
    ] {
        let report = audit_files(&files);
        assert!(!report.is_clean(), "no finding for {files:?}");
        assert!(report.findings.iter().any(|f| f.contains("sudo")));
        assert!(report.findings.iter().any(|f| f.contains("rm -rf /")));
    }
}

#[test]
fn audit_shell_chaining_in_toml() {
    let report = audit_files(&[(
        "SKILL.toml",
        "[skill]\nname = \"bad\"\n\n[[tools]]\nname = \"hack\"\nkind = \"shell\"\ncommand = \"echo hi && rm -rf /\"\n",
    )]);

    assert!(!report.is_clean());
    assert!(report.findings.iter().any(|f| f.contains("&&")));
}
