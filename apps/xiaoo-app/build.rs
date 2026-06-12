use std::env;
use std::fs;
use std::io;
use std::path::Path;
use sysinfo::System;

fn main() {
    compile_conch_proto();
    if is_cargo_install() {
        install_builtin_skills();
    }
}

fn compile_conch_proto() {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("find vendored protoc");
    std::env::set_var("PROTOC", protoc);
    tonic_build::configure()
        .build_server(false)
        .compile_protos(
            &["src/gateway/backend/conch/proto/agent.proto"],
            &["src/gateway/backend/conch/proto"],
        )
        .expect("compile conch agent proto");
}

fn is_cargo_install() -> bool {
    let cmd = get_parent_cmdline();
    if cmd.is_empty() {
        return false;
    }

    let has_cargo = cmd.iter().any(|s| {
        let lower = s.to_lowercase();
        lower == "cargo"
            || lower == "cargo.exe"
            || lower.ends_with("/cargo")
            || lower.ends_with("/cargo.exe")
            || lower.ends_with("\\cargo")
            || lower.ends_with("\\cargo.exe")
            || lower.contains(".cargo/bin/cargo")
    });

    let has_install = cmd.iter().any(|s| s == "install");

    has_cargo && has_install
}

fn get_parent_cmdline() -> Vec<String> {
    let sys = System::new_all();
    let my_pid = std::process::id();

    for (pid, process) in sys.processes() {
        if pid.as_u32() == my_pid {
            if let Some(parent) = process.parent() {
                if let Some(parent_process) = sys.processes().get(&parent) {
                    return parent_process
                        .cmd()
                        .iter()
                        .map(|s| s.to_string_lossy().to_string())
                        .collect();
                }
            }
        }
    }
    Vec::new()
}

fn install_builtin_skills() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let skills_src_dir = Path::new(&manifest_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("plugins/skills");
    println!("cargo:rerun-if-changed={}", skills_src_dir.display());

    if !skills_src_dir.exists() {
        println!("cargo:warning=Builtin skills source directory not found: {}", skills_src_dir.display());
        println!("cargo:warning=Builtin skills will NOT be installed.");
        println!("cargo:warning=This may cause security policy enforcement to be unavailable.");
        return;
    }

    // Collect all skill directories
    let skills: Vec<String> = match fs::read_dir(&skills_src_dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    };

    if skills.is_empty() {
        println!("cargo:warning=No builtin skills found in {}", skills_src_dir.display());
        return;
    }

    // Try system-level installation first
    let system_install_base = Path::new("/usr/lib/.xiaoo/skills");
    let mut system_success_skills: Vec<(String, bool)> = Vec::new(); // (skill_name, is_replacement)
    let mut system_failed_skills: Vec<String> = Vec::new();

    if let Err(e) = fs::create_dir_all(system_install_base) {
        println!("cargo:warning=Failed to create system-level skills directory {}: {}", system_install_base.display(), e);
        println!("cargo:warning=Reason: likely permission denied (requires root privileges)");
    } else {
        for skill_name in &skills {
            let src = skills_src_dir.join(skill_name);
            let dst = system_install_base.join(skill_name);

            // Check if this is a replacement
            let is_replacement = dst.exists();

            if install_skill(&src, &dst).is_ok() {
                system_success_skills.push((skill_name.clone(), is_replacement));
            } else {
                system_failed_skills.push(skill_name.clone());
            }
        }
    }

    // Print successfully installed system-level skills
    for (skill_name, is_replacement) in &system_success_skills {
        let skill_path = system_install_base.join(skill_name);
        if *is_replacement {
            println!("   Replacing {}", skill_path.display());
        } else {
            println!("  Installing {}", skill_path.display());
        }
    }

    // If all skills installed successfully to system-level, we're done
    if system_failed_skills.is_empty() && system_success_skills.len() == skills.len() {
        return;
    }

    // Determine which skills need fallback to user-level
    let fallback_skills = if !system_failed_skills.is_empty() {
        println!("cargo:warning=Failed to install {} skill(s) to system-level directory", system_failed_skills.len());
        println!("cargo:warning=Reason: permission denied (requires root privileges)");
        println!("cargo:warning=Fallback: installing to user-level directory ~/.xiaoo/skills");
        system_failed_skills.clone()
    } else {
        skills.clone()
    };

    let Ok(home) = env::var("HOME") else {
        println!("cargo:warning=Cannot determine HOME directory");
        println!("cargo:warning=Builtin skills will NOT be installed.");
        println!("cargo:warning=This may cause security policy enforcement to be unavailable.");
        return;
    };

    let user_install_base = Path::new(&home).join(".xiaoo/skills");
    if let Err(e) = fs::create_dir_all(&user_install_base) {
        println!("cargo:warning=Failed to create user skills directory {}: {}", user_install_base.display(), e);
        println!("cargo:warning=Builtin skills will NOT be installed.");
        println!("cargo:warning=This may cause security policy enforcement to be unavailable.");
        return;
    }

    let mut user_success_count = 0;
    let mut user_success_skills: Vec<(String, bool)> = Vec::new(); // (skill_name, is_replacement)
    for skill_name in &fallback_skills {
        let src = skills_src_dir.join(skill_name);
        let dst = user_install_base.join(skill_name);

        // Check if this is a replacement
        let is_replacement = dst.exists();

        if install_skill(&src, &dst).is_ok() {
            user_success_count += 1;
            user_success_skills.push((skill_name.clone(), is_replacement));
        }
    }

    for (skill_name, is_replacement) in &user_success_skills {
        let skill_path = user_install_base.join(skill_name);
        if *is_replacement {
            println!("   Replacing {}", skill_path.display());
        } else {
            println!("  Installing {}", skill_path.display());
        }
    }

    if user_success_count == 0 && !fallback_skills.is_empty() {
        println!("cargo:warning=Failed to install builtin skills to user-level directory");
        println!("cargo:warning=This may cause security policy enforcement to be unavailable.");
    }
}

fn install_skill(src: &Path, dst: &Path) -> io::Result<()> {
    if dst.exists() {
        let backup_path = format!("{}.bak", dst.display());
        let backup = Path::new(&backup_path);

        // Remove old backup if exists
        if backup.exists() {
            fs::remove_dir_all(backup)?;
        }

        // Backup existing directory
        fs::rename(dst, backup)?;

        // Copy new version
        match copy_dir_recursive(src, dst) {
            Ok(()) => {
                // Success: remove backup
                fs::remove_dir_all(backup)?;
                Ok(())
            }
            Err(e) => {
                // Failure: restore backup
                if dst.exists() {
                    let _ = fs::remove_dir_all(dst);
                }
                fs::rename(backup, dst)?;
                Err(e)
            }
        }
    } else {
        // No existing directory, just copy
        copy_dir_recursive(src, dst)
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let file_name = entry.file_name();
        let src_path = src.join(&file_name);
        let dst_path = dst.join(&file_name);

        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
