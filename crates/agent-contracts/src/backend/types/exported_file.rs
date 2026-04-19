use std::path::PathBuf;

/// The source of an exported file.
#[derive(Debug, Clone)]
pub enum ExportedFileSource {
    /// The file is accessible at a host path.
    HostPath(PathBuf),
    /// The file contents are provided as bytes.
    Bytes(Vec<u8>),
}

/// A file that has been exported from the backend.
#[derive(Debug, Clone)]
pub struct ExportedFile {
    /// Human-readable name for display purposes.
    pub file_name: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// MIME type if known.
    pub media_type: Option<String>,
    /// The source of the file data.
    pub source: ExportedFileSource,
}
