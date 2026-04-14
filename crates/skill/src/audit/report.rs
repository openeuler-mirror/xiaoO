/// Configuration for skill security audit.
#[derive(Debug, Clone)]
pub struct SkillAuditOptions {
    /// Allow script files (.sh, .bash, .ps1, shebang files).
    pub allow_scripts: bool,
    /// Maximum file size in bytes for SKILL.md / SKILL.toml (default: 512KB).
    pub max_file_size: u64,
}

impl Default for SkillAuditOptions {
    fn default() -> Self {
        Self {
            allow_scripts: false,
            max_file_size: 512 * 1024,
        }
    }
}

/// Result of auditing a skill directory.
#[derive(Debug)]
pub struct SkillAuditReport {
    pub files_scanned: usize,
    pub findings: Vec<String>,
}

impl SkillAuditReport {
    pub fn new() -> Self {
        Self {
            files_scanned: 0,
            findings: Vec::new(),
        }
    }

    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    pub fn add_finding(&mut self, finding: String) {
        self.findings.push(finding);
    }
}

impl Default for SkillAuditReport {
    fn default() -> Self {
        Self::new()
    }
}
