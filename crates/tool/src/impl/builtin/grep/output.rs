use serde::{Deserialize, Serialize};

use super::input::OutputMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepOutput {
    pub mode: OutputMode,
    #[serde(skip_serializing_if = "usize_is_zero")]
    pub num_files: usize,
    #[serde(default)]
    pub filenames: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_matches: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_offset: Option<u32>,
}

fn usize_is_zero(n: &usize) -> bool {
    *n == 0
}

impl GrepOutput {
    pub fn new(mode: OutputMode) -> Self {
        Self {
            mode,
            num_files: 0,
            filenames: Vec::new(),
            content: None,
            num_lines: None,
            num_matches: None,
            applied_limit: None,
            applied_offset: None,
        }
    }

    pub fn with_content(mut self, content: String, num_lines: usize) -> Self {
        self.content = Some(content);
        self.num_lines = Some(num_lines);
        self
    }

    pub fn with_files(mut self, filenames: Vec<String>, num_files: usize) -> Self {
        self.filenames = filenames;
        self.num_files = num_files;
        self
    }

    pub fn with_count(mut self, num_matches: usize, num_files: usize, content: String) -> Self {
        self.num_matches = Some(num_matches);
        self.num_files = num_files;
        self.content = Some(content);
        self
    }

    pub fn with_limit(mut self, limit: u32) -> Self {
        self.applied_limit = Some(limit);
        self
    }

    pub fn with_offset(mut self, offset: u32) -> Self {
        self.applied_offset = Some(offset);
        self
    }
}
