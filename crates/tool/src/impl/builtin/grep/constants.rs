//! Constants for the builtin `grep` tool.
//!
//! - `DEFAULT_HEAD_LIMIT` caps result entries when the caller omits `head_limit`.
//! - `ABSOLUTE_HARD_CAP` is the upper bound on collected lines regardless of
//!   what the caller asks for — applies even when `head_limit: 0` (unlimited)
//!   or a value above the cap is passed. This is the defense-in-depth that
//!   prevents a runaway `rg` from filling `~/.xiaoo/truncated_tool_output/`
//!   when an LLM explicitly opts into "give me everything".
//! - `DEFAULT_TIMEOUT_MS` / `MAX_TIMEOUT_MS` bound how long an `rg`/`grep`
//!   scan may run before being killed, mirroring the `bash` tool's timeout
//!   contract. Both are overridable via environment variables.

/// Default result cap applied when `head_limit` is omitted.
pub const DEFAULT_HEAD_LIMIT: u32 = 250;

/// Absolute upper bound on collected grep output, regardless of the
/// `head_limit` value the caller passes. Applies in two cases:
///
/// 1. Caller passes `head_limit: 0` (documented as "unlimited"). The
///    framework silently caps collection at this value so a wide pattern
///    on a large repo cannot push hundreds of MB through stdout and
///    into the truncation directory before the daily cleanup runs.
/// 2. Caller passes `head_limit` greater than this value. Same
///    rationale — bound the worst case.
///
/// Matches opencode's `MAX_LINES = 2000`. Callers needing more should
/// narrow `path`/`glob` or page via `offset`.
pub const ABSOLUTE_HARD_CAP: u32 = 2_000;

/// Cap on per-line output length emitted by `rg` (via `--max-columns`).
/// Lines longer than this are replaced by rg with a placeholder.
pub const RG_MAX_COLUMNS: u32 = 500;

pub const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

pub const DEFAULT_TIMEOUT_ENV_VAR: &str = "GREP_DEFAULT_TIMEOUT_MS";
pub const MAX_TIMEOUT_ENV_VAR: &str = "GREP_MAX_TIMEOUT_MS";

/// Default per-call timeout for `rg`/`grep` execution (5 minutes). Generous on
/// purpose: large monorepo scans and `content` mode with wide context can run
/// longer than the typical sub-second case, and we prefer a correct (if slow)
/// result over a premature kill. This is still a safety net against truly
/// runaway regexes and dead mounts — not a throughput target.
pub const DEFAULT_TIMEOUT_MS: u64 = 300_000;

/// Hard upper bound a caller may request via `timeout`. Kept above the default
/// so exceptionally large scans can opt into even more headroom, while still
/// guaranteeing the tool cannot hang forever.
pub const MAX_TIMEOUT_MS: u64 = 600_000;

fn read_positive_env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| match value.parse::<u64>() {
            Ok(parsed) if parsed > 0 => Some(parsed),
            _ => {
                tracing::warn!(
                    env = name,
                    value = %value,
                    "{} must be a positive integer; ignoring and falling back to the default",
                    name,
                );
                None
            }
        })
}

pub fn default_timeout_ms() -> u64 {
    read_positive_env_u64(DEFAULT_TIMEOUT_ENV_VAR).unwrap_or(DEFAULT_TIMEOUT_MS)
}

pub fn max_timeout_ms() -> u64 {
    let default_timeout = default_timeout_ms();
    let configured_max = read_positive_env_u64(MAX_TIMEOUT_ENV_VAR).unwrap_or(MAX_TIMEOUT_MS);
    configured_max.max(default_timeout)
}
