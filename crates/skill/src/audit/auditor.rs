use std::path::Path;

use super::patterns;
use super::report::{SkillAuditOptions, SkillAuditReport};

/// Audit a skill directory for security issues.
///
/// Checks for:
/// - Missing SKILL.md or SKILL.toml manifest
/// - Symlinks (rejected)
/// - Script files (blocked unless allow_scripts)
/// - High-risk patterns in file content
/// - Shell chaining operators in SKILL.toml tool commands
/// - Oversized files
pub fn audit_skill_directory(skill_dir: &Path, options: &SkillAuditOptions) -> SkillAuditReport {
    let mut report = SkillAuditReport::new();

    if !skill_dir.is_dir() {
        report.add_finding(format!("not a directory: {}", skill_dir.display()));
        return report;
    }

    // Must have SKILL.md or SKILL.toml
    let has_manifest = skill_dir.join("SKILL.md").exists() || skill_dir.join("SKILL.toml").exists();
    if !has_manifest {
        report.add_finding("missing SKILL.md or SKILL.toml manifest".into());
        return report;
    }

    audit_directory_contents(skill_dir, options, &mut report);

    report
}

fn audit_directory_contents(
    dir: &Path,
    options: &SkillAuditOptions,
    report: &mut SkillAuditReport,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            report.add_finding(format!(
                "failed to read directory '{}': {}",
                dir.display(),
                e
            ));
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        report.files_scanned += 1;

        // Reject symlinks without following them.
        if path.symlink_metadata().map_or(false, |m| m.is_symlink()) {
            report.add_finding(format!("symlink detected: {}", path.display()));
            continue;
        }

        if path.is_dir() {
            audit_directory_contents(&path, options, report);
            continue;
        }

        if !path.is_file() {
            continue;
        }

        // Check file size
        if let Ok(meta) = path.metadata() {
            if meta.len() > options.max_file_size {
                report.add_finding(format!(
                    "file exceeds size limit ({} bytes > {} bytes): {}",
                    meta.len(),
                    options.max_file_size,
                    path.display()
                ));
                continue;
            }
        }

        // Check script extensions
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if patterns::is_script_extension(ext) && !options.allow_scripts {
                report.add_finding(format!(
                    "script file blocked (allow_scripts=false): {}",
                    path.display()
                ));
                continue;
            }
        }

        // Check file content
        if let Ok(content) = std::fs::read(&path) {
            // Shebang check
            if patterns::has_shebang(&content) && !options.allow_scripts {
                report.add_finding(format!(
                    "file with shebang blocked (allow_scripts=false): {}",
                    path.display()
                ));
                continue;
            }

            // High-risk patterns (only for text files)
            if let Ok(text) = std::str::from_utf8(&content) {
                for finding in patterns::detect_high_risk_patterns(text) {
                    report.add_finding(format!("{}: {}", finding, path.display()));
                }

                // For SKILL.toml, check tool commands for shell chaining
                if path.file_name().map_or(false, |n| n == "SKILL.toml") {
                    audit_toml_tool_commands(text, report);
                }
            }
        }
    }
}

/// Check SKILL.toml tool command entries for shell chaining operators.
fn audit_toml_tool_commands(toml_content: &str, report: &mut SkillAuditReport) {
    // Simple line-based scan for command = "..." lines
    for line in toml_content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("command") {
            let rest = rest.trim();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim().trim_matches('"').trim_matches('\'');
                for finding in patterns::detect_shell_chaining(rest) {
                    report.add_finding(finding);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/skill/audit/auditor_test.rs"]
mod tests;
