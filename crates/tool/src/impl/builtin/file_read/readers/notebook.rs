use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

pub use super::super::constants::DEFAULT_MAX_NOTEBOOK_SIZE;
use super::super::output::NotebookOutput;

#[derive(Debug)]
pub enum NotebookError {
    FileTooLarge { size: u64, max_size: u64 },
    InvalidFormat(String),
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

#[allow(dead_code)]
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

    let notebook: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| NotebookError::InvalidFormat(e.to_string()))?;

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

#[allow(dead_code)]
pub fn read_notebook_from_bytes(
    file_path: &str,
    bytes: &[u8],
    max_size: Option<u64>,
) -> Result<NotebookOutput, NotebookError> {
    let file_size = bytes.len() as u64;
    let max_allowed_size = max_size.unwrap_or(DEFAULT_MAX_NOTEBOOK_SIZE);

    if file_size > max_allowed_size {
        return Err(NotebookError::FileTooLarge {
            size: file_size,
            max_size: max_allowed_size,
        });
    }

    let content = String::from_utf8_lossy(bytes);

    let notebook: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| NotebookError::InvalidFormat(e.to_string()))?;

    let cells = notebook
        .get("cells")
        .and_then(|c| c.as_array())
        .map(|arr| arr.to_vec())
        .unwrap_or_default();

    Ok(NotebookOutput {
        file_path: file_path.to_string(),
        cells,
    })
}
