//! `@`-file mention completion for the TUI chat input.
//!
//! Typing `@` starts a file mention token. The popup matches files in the
//! active workspace and the chosen path is inserted directly after the `@`.
//! Matching is fuzzy (case-insensitive subsequence fallback) so users can
//! find files whose exact names they no longer remember.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

use crate::input::Input;

pub const FILE_MENTION_MAX_CANDIDATES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMentionCandidate {
    /// Path relative to the workspace root, using `/` separators.
    pub path: String,
    /// Directories are suggested too (`@src/`) but sort after files when
    /// match quality is equal.
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMentionToken {
    /// Character index of the `@` that starts the token.
    pub start: usize,
    /// Text between `@` and the cursor.
    pub typed: String,
}

/// Extracts the `@`-mention token the cursor sits in, if any.
///
/// A token starts at an `@` that is at line start or preceded by whitespace,
/// and ends at the cursor. The search is line-local: a newline or whitespace
/// before the cursor terminates the token. Email-style `foo@bar` is ignored.
pub fn file_mention_token(value: &str, cursor: usize) -> Option<FileMentionToken> {
    let chars: Vec<char> = value.chars().collect();
    let cursor = cursor.min(chars.len());

    let mut start = cursor;
    while start > 0 {
        let prev = chars[start - 1];
        if prev.is_whitespace() {
            break;
        }
        if prev == '@' {
            let at = start - 1;
            let preceded_by_boundary = at == 0 || chars[at - 1].is_whitespace();
            if !preceded_by_boundary {
                return None; // e.g. email address
            }
            let typed: String = chars[at + 1..cursor].iter().collect();
            return Some(FileMentionToken { start: at, typed });
        }
        start -= 1;
    }
    None
}

/// Applies a file-mention pick, replacing `@typed` with `@chosen`.
pub fn apply_file_mention_pick(input: &mut Input, chosen: &str) {
    let value = input.value();
    let cursor = input.cursor();
    let Some(token) = file_mention_token(value, cursor) else {
        return;
    };
    let chars: Vec<char> = value.chars().collect();
    let before: String = chars[..token.start].iter().collect();
    let after: String = chars[cursor..].iter().collect();
    let mention = format!("@{chosen}");
    let new_value = format!("{before}{mention}{after}");
    let new_cursor = token.start + mention.chars().count();
    *input = Input::default()
        .with_value(new_value)
        .with_cursor(new_cursor);
}

/// Returns file/directory candidates matching `typed` (the text after `@`).
pub fn file_mention_candidates(
    workspace: &Path,
    typed: &str,
    max_candidates: usize,
) -> Vec<FileMentionCandidate> {
    if max_candidates == 0 {
        return Vec::new();
    }
    let typed = normalize_typed(typed);
    let entries = walk_workspace(workspace);
    let mut scored: Vec<ScoredEntry> = entries
        .into_iter()
        .filter_map(|entry| {
            let score = match_score(&typed, &entry.path, entry.is_dir)?;
            Some(ScoredEntry {
                path: entry.path,
                is_dir: entry.is_dir,
                score,
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        a.score
            .cmp(&b.score)
            .then_with(|| a.path.len().cmp(&b.path.len()))
            .then_with(|| a.path.cmp(&b.path))
    });
    scored.truncate(max_candidates);
    scored.into_iter().map(FileMentionCandidate::from).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WalkEntry {
    path: String,
    is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScoredEntry {
    path: String,
    is_dir: bool,
    score: u32,
}

impl From<ScoredEntry> for FileMentionCandidate {
    fn from(entry: ScoredEntry) -> Self {
        Self {
            path: entry.path,
            is_dir: entry.is_dir,
        }
    }
}

fn normalize_typed(typed: &str) -> String {
    typed.trim_start_matches("./").to_lowercase()
}

/// Scores lower better. `None` means no match at all.
fn match_score(typed: &str, path: &str, is_dir: bool) -> Option<u32> {
    let dir_penalty = u32::from(is_dir);
    if typed.is_empty() {
        return Some(dir_penalty);
    }
    let path_lower = path.to_lowercase();
    let basename = path_lower.rsplit('/').next().unwrap_or(&path_lower);

    if basename == typed {
        return Some(dir_penalty);
    }
    if basename.starts_with(typed) {
        return Some(2 + dir_penalty);
    }
    if basename.contains(typed) {
        return Some(4 + dir_penalty);
    }
    if path_lower.starts_with(typed) {
        return Some(8 + dir_penalty);
    }
    if path_lower.contains(typed) {
        return Some(12 + dir_penalty);
    }
    if typed.chars().count() >= 2 && is_subsequence(typed, basename) {
        return Some(20 + dir_penalty);
    }
    if typed.chars().count() >= 3 && is_subsequence(typed, &path_lower) {
        return Some(28 + dir_penalty);
    }
    None
}

/// Case-insensitive subsequence test used for fuzzy file-name matching.
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut haystack = haystack.chars();
    for want in needle.chars() {
        if !haystack.any(|c| c.to_lowercase().eq(want.to_lowercase())) {
            return false;
        }
    }
    true
}

const SKIP_DIRS: &[&str] = &[".git", ".hg", ".svn", "node_modules", "target"];

fn walk_workspace(workspace: &Path) -> Vec<WalkEntry> {
    let mut out: Vec<WalkEntry> = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((workspace.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if depth > 8 || out.len() >= 2048 {
            break;
        }
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };

        let mut entries: Vec<PathBuf> = read_dir
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        entries.sort();

        for path in entries {
            if out.len() >= 2048 {
                break;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            let is_dir = metadata.is_dir();
            if is_dir {
                if SKIP_DIRS.contains(&name) || name.starts_with('.') {
                    continue;
                }
                queue.push_back((path.clone(), depth + 1));
            }
            if let Ok(rel) = path.strip_prefix(workspace) {
                out.push(WalkEntry {
                    path: rel.to_string_lossy().replace('\\', "/"),
                    is_dir,
                });
            }
        }
    }

    out.sort_by(|a, b| a.is_dir.cmp(&b.is_dir).then_with(|| a.path.cmp(&b.path)));
    out
}

#[cfg(test)]
mod tests {
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
    }

    #[test]
    fn fuzzy_score_falls_back_to_subsequence() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("fuzz")).unwrap();
        fs::write(dir.path().join("fuzz").join("scoring_helper.rs"), "x").unwrap();
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
}
