use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SendFileInput {
    /// Absolute path to the file to send.
    pub file_path: String,
    /// Optional label/description for the file.
    #[serde(default)]
    pub label: Option<String>,
}
