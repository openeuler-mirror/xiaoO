use super::{
    append_entry, input_history_path_from_home, load_input_history_from_path,
    save_input_history_at, MAX_INPUT_HISTORY,
};
use std::path::PathBuf;

#[test]
fn append_entry_keeps_recent_limit() {
    let mut entries = Vec::new();
    for index in 0..105 {
        append_entry(&mut entries, &format!("input {index}"));
    }

    assert_eq!(entries.len(), MAX_INPUT_HISTORY);
    assert_eq!(entries.first().map(String::as_str), Some("input 5"));
    assert_eq!(entries.last().map(String::as_str), Some("input 104"));
}

#[test]
fn append_entry_ignores_blank_and_consecutive_duplicate() {
    let mut entries = vec!["hello".to_string()];

    append_entry(&mut entries, "   ");
    append_entry(&mut entries, "hello");
    append_entry(&mut entries, "world");

    assert_eq!(entries, vec!["hello".to_string(), "world".to_string()]);
}

#[test]
fn input_history_path_uses_xiaoo_home_dir() {
    let path =
        input_history_path_from_home(Some(PathBuf::from("/home/test"))).expect("history path");

    assert_eq!(path, PathBuf::from("/home/test/.xiaoo/input_history.json"));
}

#[test]
fn input_history_path_errors_without_home_dir() {
    let error = input_history_path_from_home(None).expect_err("missing home should error");

    assert!(error
        .to_string()
        .contains("unable to resolve home directory for ~/.xiaoo"));
}

#[test]
fn save_input_history_persists_to_given_history_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let history_path = temp.path().join(".xiaoo").join("input_history.json");
    let entries = vec!["first".to_string(), "second".to_string()];

    save_input_history_at(&history_path, &entries).expect("save history");

    let loaded = load_input_history_from_path(&history_path).expect("load history");
    assert_eq!(loaded, entries);
    assert!(history_path.exists());
}
