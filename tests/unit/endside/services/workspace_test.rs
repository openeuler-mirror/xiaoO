use super::resolve_dir_command;
use std::fs;

#[test]
fn resolves_relative_dir_from_current_workspace() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let nested = workspace.join("nested");
    fs::create_dir_all(&nested).expect("create dirs");

    let resolved = resolve_dir_command("/dir nested", &workspace).expect("relative workspace path");

    assert_eq!(
        resolved,
        nested.canonicalize().expect("canonical nested path")
    );
}
