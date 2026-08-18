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
    /// `true` when the result was capped at `applied_limit` and the
    /// aggregate fields (`num_matches`, `num_files`, `num_lines`) reflect
    /// ONLY the shown subset, not the full match set. The caller should
    /// NOT treat the aggregates as totals — use `offset` to page through
    /// the rest, or narrow `path`/`glob` to reduce the match count.
    ///
    /// Always set together with `applied_limit: Some(_)`; `false` (or
    /// absent) means the aggregates are complete.
    #[serde(default, skip_serializing_if = "is_false")]
    pub partial: bool,
}

fn usize_is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
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
            partial: false,
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
        // A non-None `applied_limit` means the result was capped, so the
        // aggregate fields reflect only the shown subset — flag it so
        // callers don't mistake `num_matches`/`num_files` for totals.
        self.partial = true;
        self
    }

    pub fn with_offset(mut self, offset: u32) -> Self {
        self.applied_offset = Some(offset);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `with_limit` (called when the result was capped by `apply_head_limit`)
    /// must set `partial = true` so callers don't mistake aggregate fields
    /// for totals. This is the fix for the count-mode `total_matches`
    /// regression: a capped count result now carries an explicit signal
    /// that `num_matches` is a partial sum across the shown subset only.
    #[test]
    fn with_limit_sets_partial_flag() {
        let output = GrepOutput::new(OutputMode::Count)
            .with_count(1250, 250, "file_a:5\nfile_b:3".to_string())
            .with_limit(250);
        assert!(output.partial, "partial must be true after with_limit");
        assert_eq!(
            output.applied_limit,
            Some(250),
            "applied_limit must be set together with partial"
        );
    }

    /// Without `with_limit`, `partial` stays false — aggregate fields
    /// represent the complete result and callers may treat them as totals.
    #[test]
    fn partial_defaults_to_false_when_not_capped() {
        let output =
            GrepOutput::new(OutputMode::Count).with_count(42, 3, "file_a:5\nfile_b:37".to_string());
        assert!(
            !output.partial,
            "partial must be false when the result was not capped"
        );
        assert!(output.applied_limit.is_none());
    }

    /// `partial` serializes only when true (`skip_serializing_if = is_false`
    /// — absent in JSON when false, present when true). Backwards-compat
    /// for consumers that don't know about the field: they still parse
    /// the JSON fine; consumers that DO know can read the explicit signal.
    #[test]
    fn partial_serializes_only_when_true() {
        // Capped result → JSON includes "partial": true.
        let capped = GrepOutput::new(OutputMode::Content)
            .with_content("line1\nline2".to_string(), 2)
            .with_limit(2);
        let capped_json = serde_json::to_value(&capped).unwrap();
        assert_eq!(capped_json["partial"], serde_json::Value::Bool(true));

        // Uncapped result → JSON omits "partial" entirely (default false).
        let uncapped =
            GrepOutput::new(OutputMode::Content).with_content("line1\nline2".to_string(), 2);
        let uncapped_json = serde_json::to_value(&uncapped).unwrap();
        assert!(
            uncapped_json.get("partial").is_none(),
            "partial must be omitted from JSON when false; got {:?}",
            uncapped_json.get("partial")
        );
    }
}
