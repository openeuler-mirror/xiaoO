use super::*;

#[test]
fn token_at_cursor() {
    let token = file_mention_token("fix @src/ma", 11).unwrap();
    assert_eq!(token.start, 4);
    assert_eq!(token.typed, "src/ma");
}

#[test]
fn token_at_line_start() {
    let token = file_mention_token("@read", 5).unwrap();
    assert_eq!(token.start, 0);
    assert_eq!(token.typed, "read");
}

#[test]
fn token_inside_word_is_ignored() {
    assert!(file_mention_token("a@b", 3).is_none());
}

#[test]
fn token_requires_at_sign() {
    assert!(file_mention_token("hello", 5).is_none());
}

#[test]
fn token_stops_at_space() {
    assert!(file_mention_token("@src ma", 7).is_none());
}

#[test]
fn pick_replaces_mention() {
    let mut input: Input = "@sr".into();
    apply_file_mention_pick(&mut input, "src/main.rs");
    assert_eq!(input.value(), "@src/main.rs");
    assert_eq!(input.cursor(), 12);
}

#[test]
fn pick_keeps_surrounding_text() {
    let mut input = Input::from("fix @sr please");
    input.set_cursor(7);
    apply_file_mention_pick(&mut input, "src/main.rs");
    assert_eq!(input.value(), "fix @src/main.rs please");
}

#[test]
fn empty_query_lists_workspace_entries() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("README.md"), "x").unwrap();
    fs::write(dir.path().join("src").join("main.rs"), "x").unwrap();
    let candidates = file_mention_candidates(dir.path(), "", 16);
    assert!(candidates.iter().any(|c| c.path == "src" && c.is_dir));
    assert!(candidates.iter().any(|c| c.path == "README.md"));
    // Root-level listing is one level deep: nested files stay out.
    assert!(!candidates.iter().any(|c| c.path.contains("main.rs")));
}

#[test]
fn typed_directory_enters_that_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("crates/mcp")).unwrap();
    fs::write(dir.path().join("crates/mcp/lib.rs"), "x").unwrap();
    fs::write(dir.path().join("README.md"), "x").unwrap();
    let candidates = file_mention_candidates(dir.path(), "crates/m", 16);
    assert!(candidates
        .iter()
        .any(|c| c.path == "crates/mcp" && c.is_dir));
    assert!(!candidates.iter().any(|c| c.path == "README.md"));
    assert!(!candidates.iter().any(|c| c.path.contains("lib.rs")));
}

#[test]
fn typed_nested_prefix_enters_nested_directory() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("crates/mcp/src")).unwrap();
    fs::write(dir.path().join("crates/mcp/src/lib.rs"), "x").unwrap();
    fs::write(dir.path().join("crates/mcp/Cargo.toml"), "x").unwrap();
    let candidates = file_mention_candidates(dir.path(), "crates/mcp/s", 16);
    assert!(candidates
        .iter()
        .any(|c| c.path == "crates/mcp/src" && c.is_dir));
    assert!(!candidates
        .iter()
        .any(|c| c.path.contains("Cargo.toml") || c.path.contains("lib.rs")));
}

#[test]
fn fuzzy_score_falls_back_to_subsequence() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("scoring_helper.rs"), "x").unwrap();
    let candidates = file_mention_candidates(dir.path(), "sh", 16);
    assert!(candidates.iter().any(|c| c.path.contains("scoring_helper")));
}

#[test]
fn skips_hidden_dirs() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".git").join("config"), "x").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src").join("main.rs"), "x").unwrap();
    let candidates = file_mention_candidates(dir.path(), "", 32);
    assert!(candidates.iter().any(|c| c.path == "src"));
    assert!(!candidates.iter().any(|c| c.path.contains(".git")));
}
