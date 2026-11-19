use super::*;

#[test]
fn file_mention_token_extracts_typed_text_at_word_start() {
    // (line, cursor) → expected (start, typed): the @-mention under the
    // cursor is only recognized at a word start, and the scan stops at
    // the first space.
    for (line, cursor, expected) in [
        ("fix @src/ma", 11, Some((4, "src/ma"))), // mid-line token
        ("@read", 5, Some((0, "read"))),          // token at line start
        ("a@b", 3, None),                         // @ inside a word: ignored
        ("hello", 5, None),                       // no @ sign
        ("@src ma", 7, None),                     // cursor past the space
    ] {
        assert_eq!(
            file_mention_token(line, cursor).map(|token| (token.start, token.typed)),
            expected.map(|(start, typed)| (start, typed.to_string())),
            "line {line:?} cursor {cursor}"
        );
    }
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
