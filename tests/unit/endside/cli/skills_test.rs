use super::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn copy_dir_rejects_destination_inside_source() {
    let temp = tempdir().unwrap();
    let src = temp.path().join("skills");
    fs::create_dir(&src).unwrap();
    fs::write(src.join("SKILL.md"), "test").unwrap();

    let err = copy_dir_recursive(&src, &src.join("nested")).unwrap_err();

    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}
