//! Built-in policy profiles and layered policy source resolution.
//!
//! Supports both built-in profiles and file-backed TOML policy files
//! with deterministic precedence:
//! 1. Explicit file path override
//! 2. Discovered config-backed profile from `config/cerberus-policies/`
//! 3. Built-in profile fallback (workspace-write-network-on,
//!    workspace-write-network-off, workspace-write-network-on-dev-env)

mod llm_safe;
mod minimal;
mod source;
mod strict;

pub use source::PolicySource;

use cerberus_core::policy::PolicyError;
use cerberus_core::{FsPermission, FsRule, Policy};
use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};

/// Default policy directory relative to repository root.
pub const POLICY_DIR: &str = "config/cerberus-policies";

/// Canonical built-in profile name for workspace write access with network enabled.
pub const BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON: &str = "workspace-write-network-on";
/// Canonical built-in profile name for workspace write access with network blocked.
pub const BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF: &str = "workspace-write-network-off";
/// Canonical built-in profile name for workspace write access, network enabled, and dev-tool environment variables.
pub const BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV: &str =
    "workspace-write-network-on-dev-env";

/// Built-in profile names.
pub const BUILTIN_PROFILES: &[&str] = &[
    BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF,
    BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON,
    BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV,
];

fn common_network_client_paths() -> Vec<FsRule> {
    [
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
        "/etc/host.conf",
        "/etc/gai.conf",
        "/etc/ssl/certs",
        "/etc/ca-certificates",
        "/etc/ca-certificates.conf",
        "/etc/ssl/cert.pem",
        "/etc/pki/tls/certs",
        "/etc/pki/ca-trust",
        "/usr/local/share/ca-certificates",
    ]
    .into_iter()
    .map(|path| FsRule {
        path: path.into(),
        permission: FsPermission::ReadOnly,
    })
    .collect()
}

fn canonical_builtin_profile_name(name: &str) -> Option<&'static str> {
    match name {
        BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF | "strict" => {
            Some(BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF)
        }
        BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON | "minimal" => {
            Some(BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON)
        }
        BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV | "llm-safe" => {
            Some(BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV)
        }
        _ => None,
    }
}

fn is_valid_profile_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }

    chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn validate_profile_name(name: &str) -> Result<(), PolicyError> {
    if is_valid_profile_name(name) {
        Ok(())
    } else {
        Err(PolicyError::ValidationError(format!(
            "Invalid profile name '{}': use lowercase letters, digits, and hyphens only",
            name
        )))
    }
}

/// Resolve a policy by name with layered precedence.
///
/// Resolution order:
/// 1. Check for TOML file in `config/cerberus-policies/{name}.toml`
/// 2. Fall back to built-in profile if no file found
///
/// # Errors
///
/// Returns an error if:
/// - A discovered config-backed profile exists but fails to parse or validate
/// - The profile name is unknown (not built-in and no file found)
pub fn resolve_policy(name: &str) -> Result<(Policy, PolicySource), PolicyError> {
    validate_profile_name(name)?;

    match try_load_file_policy(name) {
        Ok(Some((policy, path))) => {
            return Ok((
                policy,
                PolicySource::File {
                    name: name.to_string(),
                    path,
                },
            ));
        }
        Ok(None) => {
            // File doesn't exist, fall through to built-in
        }
        Err(e) => {
            // File exists but failed to parse/validate - propagate error explicitly
            return Err(e);
        }
    }

    if let Some(canonical_name) = canonical_builtin_profile_name(name) {
        let policy = get_builtin_profile(canonical_name).expect("canonical builtin profile");
        return Ok((
            policy,
            PolicySource::BuiltIn {
                name: canonical_name.to_string(),
            },
        ));
    }

    let available = discover_policies();
    Err(PolicyError::ParseError(format!(
        "Unknown profile: '{}'. Available profiles: {}",
        name,
        available.join(", ")
    )))
}

/// Resolve a file-backed policy, expanding relative custom paths against the current directory.
pub fn resolve_policy_file(path: &Path) -> Result<Policy, PolicyError> {
    let cwd = std::env::current_dir().map_err(|e| {
        PolicyError::ValidationError(format!("Failed to read current directory: {e}"))
    })?;

    let policy_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let policy = Policy::from_file(&policy_path)?;
    resolve_relative_custom_paths(policy, &cwd)
}

/// Try to load a policy from a TOML file in the policy directory.
///
/// Returns:
/// - `Ok(Some((policy, path)))` if file exists and parses successfully
/// - `Ok(None)` if file doesn't exist
/// - `Err(e)` if file exists but fails to parse or validate
fn try_load_file_policy(name: &str) -> Result<Option<(Policy, PathBuf)>, PolicyError> {
    let policy_path = get_policy_path(name);
    if !policy_path.exists() {
        return Ok(None);
    }
    resolve_policy_file(&policy_path).map(|policy| Some((policy, policy_path)))
}

fn resolve_relative_custom_paths(mut policy: Policy, cwd: &Path) -> Result<Policy, PolicyError> {
    for rule in &mut policy.custom_paths {
        if rule.path.is_absolute() {
            continue;
        }

        // Handle ~ home directory expansion
        let raw_path_str = rule.path.to_string_lossy();
        if raw_path_str.starts_with('~') {
            if let Some(home) = dirs::home_dir() {
                let stripped = raw_path_str.strip_prefix('~').unwrap_or(&raw_path_str);
                let suffix = if stripped.starts_with('/') {
                    stripped
                } else {
                    &raw_path_str // Fallback to original if no separator
                };
                rule.path = home.join(suffix.trim_start_matches('/'));
            }
            continue;
        }

        rule.path = resolve_path_against_cwd(cwd, &rule.path)?;
    }

    Ok(policy)
}

fn resolve_path_against_cwd(cwd: &Path, raw_path: &Path) -> Result<PathBuf, PolicyError> {
    let mut resolved = cwd.to_path_buf();

    for component in raw_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => resolved.push(part),
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err(PolicyError::ValidationError(format!(
                        "relative custom_paths escape the filesystem root: {}",
                        raw_path.display()
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(PolicyError::ValidationError(format!(
                    "invalid relative custom_paths entry: {}",
                    raw_path.display()
                )));
            }
        }
    }

    Ok(resolved)
}

/// Get the expected file path for a named policy.
fn get_policy_path(name: &str) -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_policy_dir(&cwd)
        .unwrap_or_else(|| cwd.join(POLICY_DIR))
        .join(format!("{}.toml", name))
}

/// Find the policy directory by searching upward from the given path.
pub fn find_policy_dir(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();

    loop {
        let policy_dir = current.join(POLICY_DIR);
        if policy_dir.exists() && policy_dir.is_dir() {
            return Some(policy_dir);
        }

        if !current.pop() {
            return None;
        }
    }
}

/// Get a built-in profile by name.
pub fn get_builtin_profile(name: &str) -> Option<Policy> {
    match canonical_builtin_profile_name(name)? {
        BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV => Some(llm_safe::create()),
        BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF => Some(strict::create()),
        BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON => Some(minimal::create()),
        _ => None,
    }
}

/// Get a profile by name (backward-compatible wrapper).
///
/// This function checks built-in profiles first for backward compatibility.
/// For full source tracking, use `resolve_policy()` instead.
pub fn get_profile(name: &str) -> Option<Policy> {
    get_builtin_profile(name)
}

/// Discover all available policies (built-in + file-backed).
///
/// Returns a sorted list of unique profile names that can be resolved.
pub fn discover_policies() -> Vec<String> {
    let mut profiles: Vec<String> = BUILTIN_PROFILES.iter().map(|s| s.to_string()).collect();

    if let Some(policy_dir) = find_policy_dir(&std::env::current_dir().unwrap_or_default()) {
        if let Ok(entries) = fs::read_dir(&policy_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "toml") {
                    if let Some(stem) = path.file_stem() {
                        if let Some(name) = stem.to_str() {
                            let name = name.to_string();
                            if !profiles.contains(&name) {
                                profiles.push(name);
                            }
                        }
                    }
                }
            }
        }
    }

    profiles.sort();
    profiles
}

/// List available built-in profile names.
///
/// Note: This only lists built-in profiles. Use `discover_policies()`
/// for the complete list including file-backed policies.
pub fn list_profiles() -> Vec<String> {
    BUILTIN_PROFILES.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_profiles_exist() {
        assert!(get_builtin_profile(BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON).is_some());
        assert!(get_builtin_profile(BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF).is_some());
        assert!(get_builtin_profile(BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV).is_some());
        assert!(get_builtin_profile("minimal").is_some());
        assert!(get_builtin_profile("strict").is_some());
        assert!(get_builtin_profile("llm-safe").is_some());
        assert!(get_builtin_profile("nonexistent").is_none());
    }

    #[test]
    fn test_list_profiles_returns_builtins() {
        let profiles = list_profiles();
        assert_eq!(profiles.len(), 3);
        assert!(profiles.contains(&BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON.to_string()));
        assert!(profiles.contains(&BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF.to_string()));
        assert!(profiles.contains(&BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV.to_string()));
    }

    #[test]
    fn test_resolve_builtin_policy_alias_returns_canonical_source_name() {
        let result = resolve_policy("minimal");
        assert!(result.is_ok());
        let (_, source) = result.unwrap();
        assert!(source.is_builtin());
        assert_eq!(source.name(), BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON);
    }

    #[test]
    fn test_resolve_policy_file_expands_dot_to_current_directory() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_path = temp_dir.path().join("dot-policy.toml");
        let mut file = std::fs::File::create(&policy_path).unwrap();
        writeln!(
            file,
            "[[custom_paths]]\npath = \".\"\npermission = \"readwrite\""
        )
        .unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();
        let policy = resolve_policy_file(&policy_path).expect("policy should load");
        let _ = std::env::set_current_dir(&original_dir);

        assert_eq!(policy.custom_paths.len(), 1);
        assert_eq!(policy.custom_paths[0].path, temp_dir.path());
    }

    #[test]
    fn test_resolve_policy_file_expands_relative_subpath_to_current_directory() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_path = temp_dir.path().join("subpath-policy.toml");
        let mut file = std::fs::File::create(&policy_path).unwrap();
        writeln!(
            file,
            "[[custom_paths]]\npath = \"plugins\"\npermission = \"readwrite\""
        )
        .unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();
        let policy = resolve_policy_file(&policy_path).expect("policy should load");
        let _ = std::env::set_current_dir(&original_dir);

        assert_eq!(policy.custom_paths.len(), 1);
        assert_eq!(policy.custom_paths[0].path, temp_dir.path().join("plugins"));
    }

    #[test]
    fn test_resolve_policy_file_expands_parent_directory_against_current_directory() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let base_dir = temp_dir.path().join("workspace");
        std::fs::create_dir_all(base_dir.join("nested")).unwrap();
        let policy_path = base_dir.join("nested").join("parent-policy.toml");
        let mut file = std::fs::File::create(&policy_path).unwrap();
        writeln!(
            file,
            "[[custom_paths]]\npath = \"../shared\"\npermission = \"readwrite\""
        )
        .unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(base_dir.join("nested")).unwrap();
        let policy = resolve_policy_file(&policy_path).expect("policy should load");
        let _ = std::env::set_current_dir(&original_dir);

        assert_eq!(policy.custom_paths.len(), 1);
        assert_eq!(policy.custom_paths[0].path, base_dir.join("shared"));
    }

    #[test]
    fn test_resolve_unknown_policy() {
        let result = resolve_policy("definitely-not-a-real-profile-xyz");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown profile"));
    }

    #[test]
    fn test_discover_policies_includes_builtins() {
        let profiles = discover_policies();
        assert!(profiles.contains(&BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON.to_string()));
        assert!(profiles.contains(&BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_OFF.to_string()));
        assert!(profiles.contains(&BUILTIN_PROFILE_WORKSPACE_WRITE_NETWORK_ON_DEV_ENV.to_string()));
    }

    #[test]
    fn test_legacy_builtin_aliases_resolve() {
        for name in ["minimal", "strict", "llm-safe"] {
            let result = resolve_policy(name);
            assert!(result.is_ok(), "legacy alias should resolve: {name}");
            let (_, source) = result.unwrap();
            assert!(
                source.is_builtin(),
                "legacy alias should map to builtin: {name}"
            );
        }
    }

    #[test]
    fn test_invalid_profile_names_are_rejected() {
        for name in [
            "",
            "../escape",
            "subdir/name",
            r"subdir\name",
            ".hidden",
            "-leading",
        ] {
            let result = resolve_policy(name);
            assert!(
                matches!(result, Err(PolicyError::ValidationError(_))),
                "name should be rejected: {name}"
            );
        }
    }

    #[test]
    fn test_malformed_file_policy_returns_error() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_dir = temp_dir.path().join("config/cerberus-policies");
        std::fs::create_dir_all(&policy_dir).unwrap();

        let malformed_path = policy_dir.join("malformed.toml");
        let mut file = std::fs::File::create(&malformed_path).unwrap();
        writeln!(file, "this is not valid toml [[[[").unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = resolve_policy("malformed");

        let _ = std::env::set_current_dir(&original_dir);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("parse") || err.to_string().contains("Parse"));
    }

    #[test]
    fn test_valid_file_policy_takes_precedence() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_dir = temp_dir.path().join("config/cerberus-policies");
        std::fs::create_dir_all(&policy_dir).unwrap();

        let custom_path = policy_dir.join("custom-test.toml");
        let mut file = std::fs::File::create(&custom_path).unwrap();
        writeln!(file, "[resources]\ntimeout_secs = 42").unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = resolve_policy("custom-test");

        let _ = std::env::set_current_dir(&original_dir);

        assert!(result.is_ok());
        let (policy, source) = result.unwrap();
        assert!(!source.is_builtin());
        assert_eq!(policy.timeout().as_secs(), 42);
    }

    #[test]
    fn test_resolve_policy_file_expands_tilde_to_home() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_path = temp_dir.path().join("tilde-policy.toml");
        let mut file = std::fs::File::create(&policy_path).unwrap();
        writeln!(
            file,
            "[[custom_paths]]\npath = \"~/.xiaoo/skills/xiaoo-guardian/\"\npermission = \"readexecute\""
        )
        .unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let policy = resolve_policy_file(&policy_path).expect("policy should load");

        let _ = std::env::set_current_dir(&original_dir);

        assert_eq!(policy.custom_paths.len(), 1);
        let expected_path = dirs::home_dir()
            .unwrap()
            .join(".xiaoo/skills/xiaoo-guardian/");
        assert_eq!(
            policy.custom_paths[0].path, expected_path,
            "tilde should be expanded to home directory"
        );
    }

    #[test]
    fn test_resolve_policy_file_expands_tilde_with_subpath() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_path = temp_dir.path().join("tilde-subpath-policy.toml");
        let mut file = std::fs::File::create(&policy_path).unwrap();
        writeln!(
            file,
            "[[custom_paths]]\npath = \"~/my-project/config\"\npermission = \"readwrite\""
        )
        .unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let policy = resolve_policy_file(&policy_path).expect("policy should load");

        let _ = std::env::set_current_dir(&original_dir);

        assert_eq!(policy.custom_paths.len(), 1);
        let expected_path = dirs::home_dir().unwrap().join("my-project/config");
        assert_eq!(
            policy.custom_paths[0].path, expected_path,
            "tilde with subpath should be expanded correctly"
        );
    }
}
