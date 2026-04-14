//! Notebook file reader implementation.

use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

pub use super::super::constants::DEFAULT_MAX_NOTEBOOK_SIZE;
use super::super::output::NotebookOutput;

/// Error type for notebook reading failures.
#[derive(Debug)]
pub enum NotebookError {
    /// File is too large.
    FileTooLarge { size: u64, max_size: u64 },
    /// Invalid notebook format.
    InvalidFormat(String),
    /// IO error.
    Io(std::io::Error),
}

impl std::fmt::Display for NotebookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotebookError::FileTooLarge { size, max_size } => {
                write!(
                    f,
                    "Notebook file too large: {} bytes (max: {} bytes)",
                    size, max_size
                )
            }
            NotebookError::InvalidFormat(msg) => write!(f, "Invalid notebook format: {}", msg),
            NotebookError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for NotebookError {}

impl From<std::io::Error> for NotebookError {
    fn from(err: std::io::Error) -> Self {
        NotebookError::Io(err)
    }
}

/// Reads a Jupyter notebook file (.ipynb).
///
/// A Jupyter notebook is a JSON file with the following structure:
/// ```json
/// {
///   "cells": [
///     {
///       "cell_type": "code",
///       "source": "print('hello')",
///       "execution_count": 1,
///       "outputs": []
///     },
///     {
///       "cell_type": "markdown",
///       "source": "# Title"
///     }
///   ]
/// }
/// ```
///
/// # Arguments
/// * `file_path` - Path to the notebook file
/// * `max_size` - Optional maximum file size in bytes. If None, uses DEFAULT_MAX_NOTEBOOK_SIZE.
///
/// # Returns
/// * `Ok(NotebookOutput)` containing the file path and parsed cells
/// * `Err(NotebookError)` if the file is too large, invalid, or cannot be read
pub async fn read_notebook<P: AsRef<Path>>(
    file_path: P,
    max_size: Option<u64>,
) -> Result<NotebookOutput, NotebookError> {
    let file_path = file_path.as_ref();
    let file_path_str = file_path.to_string_lossy().to_string();

    let file = File::open(file_path).await?;
    let metadata = file.metadata().await?;
    let file_size = metadata.len();

    let max_allowed_size = max_size.unwrap_or(DEFAULT_MAX_NOTEBOOK_SIZE);
    if file_size > max_allowed_size {
        return Err(NotebookError::FileTooLarge {
            size: file_size,
            max_size: max_allowed_size,
        });
    }

    let reader = BufReader::new(file);
    let mut lines_stream = reader.lines();
    let mut content = String::new();

    while let Some(line) = lines_stream.next_line().await? {
        content.push_str(&line);
    }

    // Parse the notebook JSON
    let notebook: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| NotebookError::InvalidFormat(e.to_string()))?;

    // Extract the cells array
    let cells = notebook
        .get("cells")
        .and_then(|c| c.as_array())
        .map(|arr| arr.to_vec())
        .unwrap_or_default();

    Ok(NotebookOutput {
        file_path: file_path_str,
        cells,
    })
}
