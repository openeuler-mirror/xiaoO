use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};
use tokio::fs;

pub async fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let temporary_path = path.with_extension("tmp");
    fs::write(&temporary_path, bytes).await?;
    fs::rename(temporary_path, path).await?;
    Ok(())
}

pub async fn read_json<T: DeserializeOwned>(path: &Path) -> std::io::Result<T> {
    let bytes = fs::read(path).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub async fn list_json_files(directory: &Path) -> std::io::Result<Vec<PathBuf>> {
    if fs::metadata(directory).await.is_err() {
        return Ok(Vec::new());
    }

    let mut reader = fs::read_dir(directory).await?;
    let mut files = Vec::new();
    while let Some(entry) = reader.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
