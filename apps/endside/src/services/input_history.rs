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

pub(crate) fn save_input_history(entries: &[String]) -> Result<()> {
    save_input_history_at(&input_history_path()?, entries)
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
    input_history_path_from_home(dirs::home_dir())
}

fn input_history_path_from_home(home: Option<PathBuf>) -> Result<PathBuf> {
    let home = home.context("unable to resolve home directory for ~/.xiaoo")?;
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
#[path = "../../../../tests/unit/endside/services/input_history_test.rs"]
mod tests;
