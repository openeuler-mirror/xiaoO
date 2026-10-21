use std::path::PathBuf;

/// Expand a leading `~` / `~/` prefix to the user's home directory.
///
/// Returns `None` when the value does not start with a tilde or when
/// `$HOME` is not set; other values are passed through unchanged.
pub(crate) fn expand_tilde(value: &str) -> Option<PathBuf> {
    if value == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }
    let suffix = value.strip_prefix("~/")?;
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(suffix))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/tool/impl/plugin/path_util_test.rs"]
mod tests;
