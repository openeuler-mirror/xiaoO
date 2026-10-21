//! Minimal child environments for plugin-influenced subprocesses.
//!
//! The daemon `set_var`s every keystore plaintext credential into its own
//! environment at startup (`inject_llm_secrets_into_env`), so children
//! running plugin-contributed code (exec tool commands, externalized fs
//! mutations) must start from an empty environment and re-add only what
//! they need. Single source of the allowlist so spawn sites cannot drift.

use tokio::process::Command;

/// What a wrapped helper (shell, backend CLI, sandbox launcher) needs to
/// locate binaries, expand `~`, and pick a temp directory.
const INHERITED_ENV_KEYS: &[&str] = &["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "SHELL", "USER"];

/// Clear `command`'s environment and re-add the minimal execution allowlist.
pub(crate) fn scrub_child_environment(command: &mut Command) {
    command.env_clear();
    for inherited in INHERITED_ENV_KEYS {
        if let Ok(value) = std::env::var(inherited) {
            command.env(inherited, value);
        }
    }
}
