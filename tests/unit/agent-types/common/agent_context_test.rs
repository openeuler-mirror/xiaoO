use super::*;

#[test]
fn workspace_root_string_empty_root_maps_to_none() {
    assert_eq!(workspace_root_string(std::path::Path::new("")), None);
}

#[test]
fn workspace_root_string_real_root_maps_to_lossy_string() {
    assert_eq!(
        workspace_root_string(std::path::Path::new("/ws/proj-a")),
        Some("/ws/proj-a".to_string())
    );
}
