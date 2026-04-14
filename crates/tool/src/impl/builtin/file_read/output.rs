use serde::{Deserialize, Serialize};

/// Output types for FileReadTool.
///
/// Matches the TypeScript outputSchema:
/// - text: Plain text file content
/// - image: Image file (jpeg, png, gif, webp)
/// - notebook: Jupyter notebook
/// - pdf: PDF document
/// - parts: File split into parts
/// - file_unchanged: File that was read without changes

/// Text output for plain text files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextOutput {
    /// The path to the file that was read.
    pub file_path: String,
    /// The text content of the file.
    pub content: String,
    /// Number of lines in this output.
    pub num_lines: u64,
    /// The starting line number (1-indexed).
    pub start_line: u64,
    /// Total number of lines in the file.
    pub total_lines: u64,
}

/// Image dimensions (optional).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDimensions {
    /// Original width in pixels.
    pub original_width: u32,
    /// Original height in pixels.
    pub original_height: u32,
    /// Display width in pixels (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_width: Option<u32>,
    /// Display height in pixels (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_height: Option<u32>,
}

/// Image output for image files (jpeg, png, gif, webp).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOutput {
    /// Base64 encoded image content.
    pub base64: String,
    /// Media type (e.g., "image/jpeg", "image/png", "image/gif", "image/webp").
    pub media_type: String,
    /// Original file size in bytes.
    pub original_size: u64,
    /// Image dimensions (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<ImageDimensions>,
}

/// Notebook cell representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookCell {
    /// Cell type (e.g., "code", "markdown").
    #[serde(rename = "type")]
    pub cell_type: String,
    /// Cell content.
    pub source: serde_json::Value,
    /// Execution count (for code cells).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u32>,
    /// Cell outputs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<serde_json::Value>>,
}

/// Notebook output for Jupyter notebook files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotebookOutput {
    /// The path to the notebook file that was read.
    pub file_path: String,
    /// Array of notebook cells.
    pub cells: Vec<serde_json::Value>,
}

/// PDF output for PDF documents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfOutput {
    /// The path to the PDF file that was read.
    pub file_path: String,
    /// Base64 encoded PDF content.
    pub base64: String,
    /// Original file size in bytes.
    pub original_size: u64,
}

/// Parts output file data for files split into multiple parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartsOutputFile {
    /// The path to the original file that was split.
    pub file_path: String,
    /// Original file size in bytes.
    pub original_size: u64,
    /// Number of parts the file was split into.
    pub count: u32,
    /// Directory containing the part files.
    pub output_dir: String,
}

/// Parts output for files split into multiple parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartsOutput {
    /// The file data.
    pub file: PartsOutputFile,
}

/// File unchanged output.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUnchangedOutput {
    /// The path to the file that was read without changes.
    pub file_path: String,
}

/// Enum representing all possible FileReadTool outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(tag = "type")]
pub enum FileReadOutput {
    /// Plain text file content.
    Text(TextOutput),
    /// Image file content.
    Image(ImageOutput),
    /// Jupyter notebook content.
    Notebook(NotebookOutput),
    /// PDF document content.
    Pdf(PdfOutput),
    /// File split into parts.
    Parts(PartsOutput),
    /// File read without changes.
    FileUnchanged(FileUnchangedOutput),
}

/// Output contract for FileReadTool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputContract {
    /// Description of the output.
    pub description: String,
}
