use std::path::PathBuf;

use crate::cli::config::FileConfig;
use xiaoo_api::skills::SkillRegistry;
use xiaoo_api::skills::{audit_skill_directory, FileSkillRegistry, SkillAuditOptions};
use xiaoo_shared::skills_support::SkillsConfig;

#[derive(clap::Subcommand)]
pub(super) enum SkillCommands {
    /// List all installed skills
    List,
    /// Show details of a specific skill
    Show { name: String },
    /// Run security audit on a skill directory
    Audit { path: String },
    /// Install a skill from a local directory or git URL
    Install { source: String },
    /// Remove an installed skill
    Remove { name: String },
}

pub(super) fn resolve_skills_config_from_file(file_cfg: &FileConfig) -> SkillsConfig {
    // Build complete skills_dirs with four levels
    let mut skills_dirs = Vec::new();

    // Priority 1: Project level (highest)
    skills_dirs.push(PathBuf::from(".xiaoo/skills"));

    // Priority 2: Config file user dirs (medium)
    if let Some(skills_section) = file_cfg.skills.as_ref() {
        if let Some(extra_dirs) = skills_section.dirs.as_ref() {
            for dir in extra_dirs {
                let path = PathBuf::from(dir);
                // Avoid duplicates with default dirs
                let dir_str = path.to_string_lossy();
                if dir_str != ".xiaoo/skills"
                    && !dir_str.ends_with("/.xiaoo/skills")
                    && !dir_str.ends_with("\\.xiaoo\\skills")
                    && dir_str != "/usr/lib/.xiaoo/skills"
                {
                    skills_dirs.push(path);
                }
            }
        }
    }

    // Priority 3: User level
    if let Some(home) = dirs::home_dir() {
        skills_dirs.push(home.join(".xiaoo").join("skills"));
    }

    // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
    skills_dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

    SkillsConfig {
        skills_dirs,
        allow_scripts: file_cfg
            .skills
            .as_ref()
            .and_then(|s| s.allow_scripts)
            .unwrap_or(false),
        disabled: file_cfg
            .skills
            .as_ref()
            .map(|skills| skills.disabled.iter().cloned().collect())
            .unwrap_or_default(),
        ..SkillsConfig::default()
    }
}

fn resolve_all_skills_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Priority 1: Project level (highest)
    dirs.push(PathBuf::from(".xiaoo/skills"));

    // Priority 2: Config file user dirs (medium)
    let file_cfg = FileConfig::load(None, false);
    if let Some(skills) = file_cfg.skills.as_ref() {
        if let Some(extra_dirs) = skills.dirs.as_ref() {
            for dir in extra_dirs {
                let path = PathBuf::from(dir);
                // Avoid duplicates with project/user/system dirs
                let dir_str = path.to_string_lossy();
                if dir_str != ".xiaoo/skills"
                    && !dir_str.ends_with("/.xiaoo/skills")
                    && !dir_str.ends_with("\\.xiaoo\\skills")
                    && dir_str != "/usr/lib/.xiaoo/skills"
                {
                    dirs.push(path);
                }
            }
        }
    }

    // Priority 3: User level
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".xiaoo").join("skills"));
    }

    // Priority 4: System level (lowest) - for built-in skills like xiaoo-guardian
    dirs.push(PathBuf::from("/usr/lib/.xiaoo/skills"));

    dirs
}

fn build_skills_config() -> SkillsConfig {
    let skills_dirs = resolve_all_skills_dirs();

    // Get allow_scripts from config file
    let file_cfg = FileConfig::load(None, false);
    let allow_scripts = file_cfg
        .skills
        .as_ref()
        .and_then(|s| s.allow_scripts)
        .unwrap_or(false);
    let disabled = file_cfg
        .skills
        .as_ref()
        .map(|skills| skills.disabled.iter().cloned().collect())
        .unwrap_or_default();

    SkillsConfig {
        skills_dirs,
        allow_scripts,
        disabled,
        ..SkillsConfig::default()
    }
}

fn project_skills_dir() -> PathBuf {
    PathBuf::from(".xiaoo/skills")
}

fn user_skills_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".xiaoo").join("skills"))
}

fn system_skills_dir() -> PathBuf {
    PathBuf::from("/usr/lib/.xiaoo/skills")
}

fn default_skills_config() -> SkillsConfig {
    build_skills_config()
}

pub(super) fn handle_skill_command(command: SkillCommands) {
    match command {
        SkillCommands::List => {
            let registry = FileSkillRegistry::new(&default_skills_config());
            let skills = registry.list_skills();
            if skills.is_empty() {
                println!("No skills installed.");
                let dirs = resolve_all_skills_dirs();
                for d in &dirs {
                    println!("  Skills directory: {}", d.display());
                }
                return;
            }
            println!("{:<20} {}", "NAME", "DESCRIPTION");
            println!("{:<20} {}", "----", "-----------");
            for s in &skills {
                println!("{:<20} {}", s.skill_id, s.description);
            }
            println!("\n{} skill(s) found.", skills.len());
        }
        SkillCommands::Show { name } => {
            let registry = FileSkillRegistry::new(&default_skills_config());
            match registry.get_skill(&name) {
                Some(spec) => {
                    println!("Skill: {}", spec.skill_id());
                    println!("Description: {}", spec.description());
                    if !spec.arguments().is_empty() {
                        println!("Arguments: {}", spec.arguments().join(", "));
                    }
                    if let Some(hint) = spec.argument_hint() {
                        println!("Argument hint: {}", hint);
                    }
                    println!("Context: {:?}", spec.context());
                    println!("User invocable: {}", spec.user_invocable());
                    if let Some(loc) = spec.location() {
                        println!("Location: {}", loc.display());
                    }
                    println!("\n--- Prompt ---\n{}", spec.full_prompt());
                }
                None => {
                    eprintln!("Skill '{}' not found.", name);
                    std::process::exit(1);
                }
            }
        }
        SkillCommands::Audit { path } => {
            let dir = PathBuf::from(&path);
            if !dir.is_dir() {
                eprintln!("Not a directory: {}", path);
                std::process::exit(1);
            }
            let report = audit_skill_directory(&dir, &SkillAuditOptions::default());
            println!("Audited: {}", dir.display());
            println!("Files scanned: {}", report.files_scanned);
            if report.is_clean() {
                println!("Result: CLEAN");
            } else {
                println!("Result: {} issue(s) found:", report.findings.len());
                for (i, f) in report.findings.iter().enumerate() {
                    println!("  {}. {}", i + 1, f);
                }
                std::process::exit(1);
            }
        }
        SkillCommands::Install { source } => {
            let is_git = source.ends_with(".git")
                || source.starts_with("https://")
                || source.starts_with("http://")
                || source.starts_with("git@")
                || source.starts_with("file://");

            // Extract skill name first (before cloning/downloading)
            let skill_name = if is_git {
                extract_repo_name(&source)
            } else {
                let p = PathBuf::from(&source);
                p.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .strip_suffix(".git")
                    .unwrap_or(p.file_name().and_then(|n| n.to_str()).unwrap_or("unknown"))
                    .to_string()
            };

            if skill_name.contains("..") || skill_name.contains('/') || skill_name.contains('\\') {
                eprintln!("Invalid skill name: {}", skill_name);
                std::process::exit(1);
            }

            // Check all skill directories for existing skill BEFORE cloning
            let project_dest = project_skills_dir().join(&skill_name);
            let user_dest = user_skills_dir().as_ref().map(|d| d.join(&skill_name));
            let system_dest = system_skills_dir().join(&skill_name);

            // Check config file directories
            let config_dests = {
                let file_cfg = FileConfig::load(None, false);
                file_cfg
                    .skills
                    .as_ref()
                    .and_then(|s| s.dirs.as_ref())
                    .map(|dirs| {
                        dirs.iter()
                            .map(|d| PathBuf::from(d).join(&skill_name))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };

            // Check project level first (highest priority)
            if project_dest.exists() {
                eprintln!(
                    "Skill '{}' already installed at {} (project level, highest priority)",
                    skill_name,
                    project_dest.display()
                );
                std::process::exit(1);
            }

            // Check config file directories (medium priority)
            for config_dest in &config_dests {
                if config_dest.exists() {
                    eprintln!(
                        "Skill '{}' already installed at {} (config directory, medium priority)",
                        skill_name,
                        config_dest.display()
                    );
                    std::process::exit(1);
                }
            }

            // Check user level
            if let Some(ref user_d) = user_dest {
                if user_d.exists() {
                    eprintln!(
                        "Skill '{}' already installed at {} (user level)",
                        skill_name,
                        user_d.display()
                    );
                    std::process::exit(1);
                }
            }

            // Check system level (lowest priority, for built-in skills only)
            if system_dest.exists() {
                eprintln!(
                    "Skill '{}' already installed at {} (system level, built-in skill)",
                    skill_name,
                    system_dest.display()
                );
                std::process::exit(1);
            }

            // Now clone/copy the source
            let src_dir = if is_git {
                let tmp = std::env::temp_dir().join(&skill_name);
                let _ = std::fs::remove_dir_all(&tmp);
                println!("Cloning {} ...", source);
                let status = std::process::Command::new("git")
                    .args([
                        "clone",
                        "--depth",
                        "1",
                        &source,
                        tmp.to_str().unwrap_or("."),
                    ])
                    .status();
                match status {
                    Ok(s) if s.success() => {}
                    Ok(s) => {
                        eprintln!("git clone failed: {}", s);
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("Failed to run git: {}", e);
                        std::process::exit(1);
                    }
                }
                let _ = std::fs::remove_dir_all(tmp.join(".git"));
                tmp
            } else {
                let p = PathBuf::from(&source);
                if !p.is_dir() {
                    eprintln!("Not a directory: {}", source);
                    std::process::exit(1);
                }
                p
            };

            // Validate that source directory contains a valid skill (SKILL.md or SKILL.toml)
            let has_manifest =
                src_dir.join("SKILL.md").exists() || src_dir.join("SKILL.toml").exists();
            if !has_manifest {
                eprintln!("Error: Source directory is not a valid skill directory.");
                eprintln!("A valid skill directory must contain either SKILL.md or SKILL.toml.");
                if is_git {
                    let _ = std::fs::remove_dir_all(&src_dir);
                }
                std::process::exit(1);
            }

            // Install to user directory by default
            // Users can manually copy to project level or config directories to override
            // System level (/usr/lib/.xiaoo/skills) is reserved for built-in skills only
            let dest = user_dest.unwrap_or_else(|| project_skills_dir().join(&skill_name));

            // Audit is currently disabled by default; use `xiaoo skill audit <path>` for manual checks.

            if let Err(e) = copy_dir_recursive(&src_dir, &dest) {
                eprintln!("Failed to install: {}", e);
                if is_git {
                    let _ = std::fs::remove_dir_all(&src_dir);
                }
                std::process::exit(1);
            }
            if is_git {
                let _ = std::fs::remove_dir_all(&src_dir);
            }
            println!("Installed skill '{}' to {}", skill_name, dest.display());
        }
        SkillCommands::Remove { name } => {
            if name.contains("..") || name.contains('/') || name.contains('\\') {
                eprintln!("Invalid skill name: {}", name);
                std::process::exit(1);
            }

            let project_dir = project_skills_dir().join(&name);
            let user_dir = user_skills_dir().as_ref().map(|d| d.join(&name));
            let system_dir = system_skills_dir().join(&name);

            // Get config file directories
            let config_dirs = {
                let file_cfg = FileConfig::load(None, false);
                file_cfg
                    .skills
                    .as_ref()
                    .and_then(|s| s.dirs.as_ref())
                    .map(|dirs| {
                        dirs.iter()
                            .map(|d| PathBuf::from(d).join(&name))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            };

            // Priority: remove from highest priority level first
            // 1. Project level (highest)
            if project_dir.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&project_dir) {
                    eprintln!("Failed to remove from project: {}", e);
                    std::process::exit(1);
                }
                println!(
                    "Removed skill '{}' from {} (project level, highest priority).",
                    name,
                    project_dir.display()
                );

                // Warn if config dirs still exist
                for config_dir in &config_dirs {
                    if config_dir.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (config directory).",
                            name,
                            config_dir.display()
                        );
                    }
                }

                // Warn if user level still exists
                if let Some(ref user_d) = user_dir {
                    if user_d.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (user level).",
                            name,
                            user_d.display()
                        );
                    }
                }

                // Warn if system level still exists
                if system_dir.is_dir() {
                    eprintln!(
                        "Warning: Skill '{}' still exists at {} (system level, built-in skill).",
                        name,
                        system_dir.display()
                    );
                }
                return;
            }

            // 2. Config directories (medium)
            for config_dir in &config_dirs {
                if config_dir.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(config_dir) {
                        eprintln!("Failed to remove from config directory: {}", e);
                        std::process::exit(1);
                    }
                    println!(
                        "Removed skill '{}' from {} (config directory).",
                        name,
                        config_dir.display()
                    );

                    // Warn if other config dirs or user/system still exist
                    for other_config_dir in &config_dirs {
                        if other_config_dir != config_dir && other_config_dir.is_dir() {
                            eprintln!(
                                "Warning: Skill '{}' still exists at {} (other config directory).",
                                name,
                                other_config_dir.display()
                            );
                        }
                    }

                    if let Some(ref user_d) = user_dir {
                        if user_d.is_dir() {
                            eprintln!(
                                "Warning: Skill '{}' still exists at {} (user level).",
                                name,
                                user_d.display()
                            );
                        }
                    }

                    if system_dir.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (system level, built-in skill).",
                            name,
                            system_dir.display()
                        );
                    }
                    return;
                }
            }

            // 3. User level
            if let Some(ref user_d) = user_dir {
                if user_d.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(user_d) {
                        eprintln!("Failed to remove from user directory: {}", e);
                        std::process::exit(1);
                    }
                    println!(
                        "Removed skill '{}' from {} (user level).",
                        name,
                        user_d.display()
                    );

                    // Warn if system level still exists
                    if system_dir.is_dir() {
                        eprintln!(
                            "Warning: Skill '{}' still exists at {} (system level, built-in skill).",
                            name,
                            system_dir.display()
                        );
                    }
                    return;
                }
            }

            // 4. System level (built-in skills only - requires root privileges to remove)
            if system_dir.is_dir() {
                eprintln!(
                    "Skill '{}' is a built-in skill at {} (system level).",
                    name,
                    system_dir.display()
                );
                eprintln!("Built-in skills require root privileges to remove.");
                eprintln!("To remove: sudo rm -rf {}", system_dir.display());
                std::process::exit(1);
            }

            // Skill not found anywhere
            eprintln!("Skill '{}' not found in any skills directory.", name);
            eprintln!("Checked directories:");
            eprintln!(
                "  - {} (project level, highest priority)",
                project_dir.display()
            );
            for config_dir in &config_dirs {
                eprintln!("  - {} (config directory)", config_dir.display());
            }
            if let Some(ref user_d) = user_dir {
                eprintln!("  - {} (user level)", user_d.display());
            }
            eprintln!(
                "  - {} (system level, built-in skills)",
                system_dir.display()
            );
            std::process::exit(1);
        }
    }
}

fn extract_repo_name(url: &str) -> String {
    let name = url.trim_end_matches('/').rsplit('/').next().unwrap_or(url);
    let name = name.rsplit(':').next().unwrap_or(name);
    let name = name.rsplit('/').next().unwrap_or(name);
    let name = name.strip_suffix(".git").unwrap_or(name);
    if name.is_empty() {
        format!("skill-{}", std::process::id())
    } else {
        name.to_string()
    }
}

fn copy_dir_recursive(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    reject_nested_copy(src, dest)?;
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dst = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), dst)?;
        }
    }
    Ok(())
}

fn reject_nested_copy(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    let src = src.canonicalize()?;
    let dest_parent = dest.parent().unwrap_or_else(|| std::path::Path::new("."));
    let dest_name = dest.file_name().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination has no name")
    })?;
    let dest_abs = dest_parent
        .canonicalize()
        .unwrap_or_else(|_| dest_parent.to_path_buf())
        .join(dest_name);

    if dest_abs.starts_with(&src) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "destination must not be inside source directory",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn copy_dir_rejects_destination_inside_source() {
        let temp = tempdir().unwrap();
        let src = temp.path().join("skills");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("SKILL.md"), "test").unwrap();

        let err = copy_dir_recursive(&src, &src.join("nested")).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
