use super::expand_path_from_base;
use std::path::Path;

#[test]
fn expands_relative_paths_from_workspace_root() {
    let expanded = expand_path_from_base("src/main.rs", Path::new("/tmp/workspace"));
    assert_eq!(expanded, "/tmp/workspace/src/main.rs");
}

#[test]
fn keeps_absolute_paths_unchanged() {
    let expanded = expand_path_from_base("/tmp/workspace/src/main.rs", Path::new("/ignored"));
    assert_eq!(expanded, "/tmp/workspace/src/main.rs");
}
