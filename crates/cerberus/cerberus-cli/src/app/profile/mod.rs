//! Profile management functionality.

use crate::app::error::CliError;
use crate::profiles::{self, PolicySource};
use cerberus_core::Policy;

/// List all available profiles (built-in + file-backed).
pub fn list_profiles() -> Vec<String> {
    profiles::discover_policies()
}

/// List built-in profiles only.
pub fn list_builtin_profiles() -> Vec<String> {
    profiles::list_profiles()
}

/// Get a profile by name (built-in only, for backward compatibility).
pub fn get_profile(name: &str) -> Option<Policy> {
    profiles::get_profile(name)
}

/// Resolve a profile with source tracking.
///
/// Returns the policy and its source (built-in or file-backed).
pub fn resolve_profile_with_source(name: &str) -> Result<(Policy, PolicySource), CliError> {
    profiles::resolve_policy(name).map_err(|e| CliError::ConfigError(e.to_string()))
}

/// Validate a profile name.
pub fn validate_profile(name: &str) -> Result<(), CliError> {
    match profiles::resolve_policy(name) {
        Ok(_) => Ok(()),
        Err(e) => Err(CliError::ConfigError(e.to_string())),
    }
}

/// Discover all policies with their sources.
///
/// Returns a list of (name, source) tuples for all available profiles.
pub fn discover_policies_with_sources() -> Vec<(String, PolicySource)> {
    let mut result = Vec::new();

    if let Some(policy_dir) =
        profiles::find_policy_dir(&std::env::current_dir().unwrap_or_default())
    {
        if let Ok(entries) = std::fs::read_dir(&policy_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "toml") {
                    if let Some(stem) = path.file_stem() {
                        if let Some(name) = stem.to_str() {
                            let name = name.to_string();
                            if !result.iter().any(|(n, _)| n == &name) {
                                result.push((
                                    name.clone(),
                                    PolicySource::File {
                                        name,
                                        path: path.clone(),
                                    },
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    for name in profiles::BUILTIN_PROFILES {
        if !result.iter().any(|(n, _)| n == name) {
            result.push((
                name.to_string(),
                PolicySource::BuiltIn {
                    name: name.to_string(),
                },
            ));
        }
    }

    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_policies_with_sources_includes_builtins() {
        let profiles = discover_policies_with_sources();

        let network_on = profiles
            .iter()
            .find(|(n, _)| n == "workspace-write-network-on");
        assert!(network_on.is_some());
        let (_, source) = network_on.unwrap();
        assert!(source.is_builtin());

        let network_off = profiles
            .iter()
            .find(|(n, _)| n == "workspace-write-network-off");
        assert!(network_off.is_some());
        let (_, source) = network_off.unwrap();
        assert!(source.is_builtin());

        let dev_env = profiles
            .iter()
            .find(|(n, _)| n == "workspace-write-network-on-dev-env");
        assert!(dev_env.is_some());
        let (_, source) = dev_env.unwrap();
        assert!(source.is_builtin());
    }

    #[test]
    fn test_resolve_profile_with_source_builtins() {
        let result = resolve_profile_with_source("minimal");
        assert!(result.is_ok());
        let (_, source) = result.unwrap();
        assert!(source.is_builtin());
        assert_eq!(source.name(), "workspace-write-network-on");
    }

    #[test]
    fn test_resolve_profile_with_source_file_backed_policy() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_dir = temp_dir.path().join("config/cerberus-policies");
        std::fs::create_dir_all(&policy_dir).unwrap();

        let policy_path = policy_dir.join("custom-file-policy.toml");
        let mut file = std::fs::File::create(&policy_path).unwrap();
        writeln!(
            file,
            "[[custom_paths]]\npath = \"/tmp\"\npermission = \"readwrite\""
        )
        .unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = resolve_profile_with_source("custom-file-policy");

        let _ = std::env::set_current_dir(&original_dir);
        assert!(result.is_ok());
        let (policy, source) = result.unwrap();
        assert!(source.is_file());
        assert_eq!(source.name(), "custom-file-policy");
        assert_eq!(policy.custom_paths.len(), 1);
    }

    #[test]
    fn test_discover_policies_with_sources_includes_repo_root_file_profiles() {
        let profiles = discover_policies_with_sources();

        for expected in [
            "repo-root-write-network-off",
            "repo-root-write-network-on",
            "repo-root-write-network-on-dev-env",
        ] {
            let profile = profiles.iter().find(|(n, _)| n == expected);
            assert!(profile.is_some(), "missing profile: {}", expected);
            let (_, source) = profile.unwrap();
            assert!(source.is_file(), "{} should be file-backed", expected);
        }
    }

    #[test]
    fn test_resolve_profile_with_source_unknown() {
        let result = resolve_profile_with_source("definitely-not-a-real-profile-xyz");
        assert!(result.is_err());
    }

    #[test]
    fn test_policy_source_display() {
        let builtin = PolicySource::BuiltIn {
            name: "workspace-write-network-on".to_string(),
        };
        assert_eq!(
            format!("{}", builtin),
            "built-in:workspace-write-network-on"
        );

        let file = PolicySource::File {
            name: "custom".to_string(),
            path: std::path::PathBuf::from("/path/to/custom.toml"),
        };
        let display = format!("{}", file);
        assert!(display.contains("file:custom"));
        assert!(display.contains("/path/to/custom.toml"));
    }

    #[test]
    fn test_policy_source_helpers() {
        let builtin = PolicySource::BuiltIn {
            name: "workspace-write-network-on".to_string(),
        };
        assert!(builtin.is_builtin());
        assert!(!builtin.is_file());
        assert!(builtin.path().is_none());

        let file = PolicySource::File {
            name: "custom".to_string(),
            path: std::path::PathBuf::from("/path/to/custom.toml"),
        };
        assert!(!file.is_builtin());
        assert!(file.is_file());
        assert!(file.path().is_some());
    }

    #[test]
    fn test_discover_policies_with_sources_prefers_file_over_builtin_on_name_collision() {
        use std::io::Write;

        let _guard = crate::test_support::lock_cwd();

        let temp_dir = tempfile::tempdir().unwrap();
        let policy_dir = temp_dir.path().join("config/cerberus-policies");
        std::fs::create_dir_all(&policy_dir).unwrap();

        let colliding = policy_dir.join("workspace-write-network-on.toml");
        let mut file = std::fs::File::create(&colliding).unwrap();
        writeln!(file, "[resources]\ntimeout_secs = 42").unwrap();
        drop(file);

        let original_dir = crate::test_support::current_dir_or_manifest_dir();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let profiles = discover_policies_with_sources();
        let resolved = resolve_profile_with_source("workspace-write-network-on").unwrap();

        let _ = std::env::set_current_dir(&original_dir);

        let listed = profiles
            .iter()
            .find(|(name, _)| name == "workspace-write-network-on")
            .expect("workspace-write-network-on should be listed");

        assert!(listed.1.is_file());
        assert!(resolved.1.is_file());
    }
}
