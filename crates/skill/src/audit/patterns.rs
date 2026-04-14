/// High-risk patterns to detect in skill files.
static HIGH_RISK_PATTERNS: &[(&str, &str)] = &[
    (r"rm\s+-rf\s+/", "destructive: rm -rf /"),
    (r"rm\s+-rf\s+~/", "destructive: rm -rf ~/"),
    (r"sudo\s+", "privilege escalation: sudo"),
    (r"curl\s+.*\|\s*sh", "remote code execution: curl | sh"),
    (r"curl\s+.*\|\s*bash", "remote code execution: curl | bash"),
    (r"wget\s+.*\|\s*sh", "remote code execution: wget | sh"),
    (r"wget\s+.*\|\s*bash", "remote code execution: wget | bash"),
    (r"mkfifo\s+.*nc\s+", "reverse shell: netcat with fifo"),
    (r":\(\)\{.*\};\s*:", "fork bomb"),
    (r">\s*/dev/sd[a-z]", "raw disk write"),
    (r"dd\s+.*of=/dev/", "raw disk write: dd"),
    (r"chmod\s+777\s+/", "dangerous permissions: chmod 777 /"),
    (r"eval\s*\(", "eval injection risk"),
];

/// Shell chaining operators that may be used to inject commands.
static SHELL_CHAINING_PATTERNS: &[&str] = &["&&", "||", ";", "|", "`"];

/// Script file extensions to block (unless allow_scripts is true).
static SCRIPT_EXTENSIONS: &[&str] = &[
    "sh", "bash", "zsh", "fish", "ps1", "psm1", "psd1", "bat", "cmd",
];

/// Check if a file has a script extension.
pub fn is_script_extension(ext: &str) -> bool {
    SCRIPT_EXTENSIONS
        .iter()
        .any(|s| s.eq_ignore_ascii_case(ext))
}

/// Check file content for high-risk patterns. Returns findings.
pub fn detect_high_risk_patterns(content: &str) -> Vec<String> {
    let mut findings = Vec::new();

    for (pattern, description) in HIGH_RISK_PATTERNS {
        if let Ok(re) = regex_lite::Regex::new(pattern) {
            if re.is_match(content) {
                findings.push(format!("high-risk pattern detected: {}", description));
            }
        }
    }

    findings
}

/// Check if a tool command contains shell chaining operators.
pub fn detect_shell_chaining(command: &str) -> Vec<String> {
    let mut findings = Vec::new();

    for op in SHELL_CHAINING_PATTERNS {
        if command.contains(op) {
            findings.push(format!(
                "shell chaining operator '{}' in tool command: {}",
                op,
                command.chars().take(80).collect::<String>()
            ));
        }
    }

    findings
}

/// Check if file content starts with a shebang (#!).
pub fn has_shebang(content: &[u8]) -> bool {
    content.starts_with(b"#!")
}
