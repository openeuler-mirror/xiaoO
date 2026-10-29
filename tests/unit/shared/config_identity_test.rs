use std::io::Write;

use crate::config_identity::{hex_sha256, ConfigFileIdentity};

#[test]
fn identity_hashes_file_content_and_reports_canonical_path() {
    let mut file = tempfile::NamedTempFile::new().expect("temp config file");
    file.write_all(b"llm.model = \"m1\"\n")
        .expect("write temp config");
    let path = file.path().to_path_buf();

    let identity = ConfigFileIdentity::from_path(&path).expect("identity should compute");
    let canonical = std::fs::canonicalize(&path).expect("canonicalize in test");
    assert_eq!(identity.path, canonical.display().to_string());
    assert_eq!(identity.sha256, hex_sha256(b"llm.model = \"m1\"\n"));
}

/// 同内容不同路径 → 摘要相同、路径不同；相等性只由摘要决定。
#[test]
fn same_content_different_paths_shares_sha256() {
    let dir = tempfile::tempdir().expect("temp dir");
    let a = dir.path().join("a.toml");
    let b = dir.path().join("b.toml");
    std::fs::write(&a, b"[llm]\nmodel = \"m\"\n").expect("write a");
    std::fs::write(&b, b"[llm]\nmodel = \"m\"\n").expect("write b");

    let ia = ConfigFileIdentity::from_path(&a).expect("identity a");
    let ib = ConfigFileIdentity::from_path(&b).expect("identity b");

    assert_eq!(ia.sha256, ib.sha256);
    assert_ne!(ia.path, ib.path);
    assert_eq!(ia, ib);
}

#[test]
fn missing_file_is_an_error() {
    let missing = tempfile::tempdir()
        .expect("temp dir")
        .path()
        .join("no-such-config.toml");
    assert!(ConfigFileIdentity::from_path(&missing).is_err());
}
