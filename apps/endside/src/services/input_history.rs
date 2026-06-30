use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) const MAX_INPUT_HISTORY: usize = 100;

const HISTORY_VERSION: u32 = 1;
const HISTORY_FILE_NAME: &str = "input_history.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InputHistoryFile {
    version: u32,
    #[serde(default)]
    entries: Vec<String>,
}

impl Default for InputHistoryFile {
    fn default() -> Self {
        Self {
            version: HISTORY_VERSION,
            entries: Vec::new(),
        }
    }
}

pub(crate) fn load_input_history() -> Result<Vec<String>> {
    load_input_history_from_path(&input_history_path()?)
}

pub(crate) fn append_input_history(input: &str) -> Result<Vec<String>> {
    let path = input_history_path()?;
    append_input_history_at(&path, input)
}

fn load_input_history_from_path(path: &Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read input history {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut entries = match serde_json::from_str::<InputHistoryFile>(&content) {
        Ok(file) => file.entries,
        Err(file_error) => serde_json::from_str::<Vec<String>>(&content).with_context(|| {
            format!(
                "failed to parse input history {}: {file_error}",
                path.display()
            )
        })?,
    };
    normalize_entries(&mut entries);
    Ok(entries)
}

fn append_input_history_at(path: &Path, input: &str) -> Result<Vec<String>> {
    let mut entries = load_input_history_from_path(path)?;
    append_entry(&mut entries, input);
    save_input_history_at(path, &entries)?;
    Ok(entries)
}

pub(crate) fn append_entry(entries: &mut Vec<String>, input: &str) {
    if input.trim().is_empty() {
        return;
    }

    if entries.last().is_some_and(|entry| entry == input) {
        return;
    }

    entries.push(input.to_string());
    truncate_to_limit(entries);
}

fn save_input_history_at(path: &Path, entries: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create input history dir {}", parent.display()))?;
    }

    let file = InputHistoryFile {
        version: HISTORY_VERSION,
        entries: entries.to_vec(),
    };
    let content = serde_json::to_string_pretty(&file)?;
    fs::write(&path, content)
        .with_context(|| format!("failed to write input history {}", path.display()))
}

fn input_history_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("unable to resolve home directory for ~/.xiaoo")?;
    Ok(home.join(".xiaoo").join(HISTORY_FILE_NAME))
}

fn normalize_entries(entries: &mut Vec<String>) {
    entries.retain(|entry| !entry.trim().is_empty());
    truncate_to_limit(entries);
}

fn truncate_to_limit(entries: &mut Vec<String>) {
    let excess = entries.len().saturating_sub(MAX_INPUT_HISTORY);
    if excess > 0 {
        entries.drain(0..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_entry, append_input_history_at, input_history_path, load_input_history_from_path,
        MAX_INPUT_HISTORY,
    };

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
        let path = input_history_path().expect("history path");
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("input_history.json")
        );
        assert_eq!(
            path.parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str()),
            Some(".xiaoo")
        );
    }

    #[test]
    fn append_input_history_persists_to_given_history_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let history_path = temp.path().join(".xiaoo").join("input_history.json");

        append_input_history_at(&history_path, "first").expect("append first");
        append_input_history_at(&history_path, "second").expect("append second");

        let loaded = load_input_history_from_path(&history_path).expect("load history");
        assert_eq!(loaded, vec!["first".to_string(), "second".to_string()]);
        assert!(history_path.exists());
    }
}
