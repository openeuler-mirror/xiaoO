//! Single source of truth for the `rg` / `grep` command lines used by every
//! content-search path in xiaoO.
//!
//! Three call sites search file contents:
//!
//! 1. the builtin `grep` tool (crate `tool`), which streams `rg` through
//!    `OperationExec::exec_streaming`,
//! 2. [`crate::backends::local`]'s `OperationSearch::grep`, which runs `rg`
//!    through its own exec plane,
//! 3. the E2B backend's `OperationSearch::grep`, which runs the same command
//!    inside the sandbox shell.
//!
//! They used to build their own argv (`rg` for the tool, `grep -r -H` for
//! E2B, a hand-written recursive walk + `str::contains` for the local
//! backend), which drifted apart. This module owns the argv, the exit-code
//! policy, and the entry-shape helpers so a change lands in all three at
//! once.
//!
//! `rg` is the primary binary; `grep` is the fallback for environments that
//! ship GNU grep but no ripgrep (E2B's stock template, SWE-bench containers).
//! [`GrepCommandSpec::rg_args`] and [`GrepCommandSpec::grep_args`] mirror each
//! other flag for flag; the documented degradations are `--type`, multiline,
//! and [`GrepOutputMode::CountMatches`] (grep has no match count, so it falls
//! back to `-c`, which counts matching *lines*).
//!
//! [`collect_entries`] sorts by backend path (then line number) so that
//! `head_limit` always truncates the same prefix: ripgrep traverses in
//! parallel and its raw output order is not stable.
//!
//! Three grep-specific details are handled here rather than at the call sites:
//! path-style globs (`src/*.rs`) are prefiltered by their basename for grep
//! but narrowed exactly by [`collect_entries`], while *negated* path globs are
//! only post-filtered (a basename form would over-exclude). Content parsing
//! understands both the match (`:`) and context (`-`) separators plus the
//! `--` group separator; the grep fallback has no `--max-columns` equivalent,
//! so [`collect_entries`] applies rg's long-line placeholder to content text
//! as well.

use agent_contracts::backend::capability::search::{GrepMode, GrepRequest};
use agent_contracts::backend::OperationError;
use std::path::Path;

/// Primary search binary.
pub const RG_BINARY: &str = "rg";
/// Fallback search binary.
pub const GREP_BINARY: &str = "grep";

/// Ceiling, in bytes, on the text of one emitted `Content` line.
///
/// `rg` enforces it with `--max-columns`: a longer line is replaced by
/// `[Omitted long matching line]` (or `[Omitted long context line]` for a
/// context line). GNU `grep` has no equivalent flag and prints the whole line,
/// so [`collect_entries`] applies the same ceiling when it formats content
/// entries; callers that consume a binary's raw stdout (the `grep` tool's
/// streaming path) are outside that guarantee.
pub const RG_MAX_COLUMNS: u32 = 500;

/// Text `rg` prints instead of a matching line longer than [`RG_MAX_COLUMNS`].
const OMITTED_LONG_MATCH_LINE: &str = "[Omitted long matching line]";
/// Text `rg` prints instead of a context line longer than [`RG_MAX_COLUMNS`].
const OMITTED_LONG_CONTEXT_LINE: &str = "[Omitted long context line]";

/// Hard ceiling on the stdout a single backend search may capture.
///
/// The operation contract is not streaming, so the whole result lands in
/// memory before it can be filtered/sorted. This ceiling turns the worst case
/// (a one-character pattern over a huge tree) into a readable error instead of
/// an OOM, which in a daemon is not recoverable.
pub const MAX_SEARCH_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Hard ceiling on the number of candidate result lines a single backend
/// search may process. Structural non-results are not counted: `grep -rc`
/// emits `path:0` for every visited file, and context output has bare `--`
/// group separators. Sorting needs the whole set, so this bounds the
/// amplification from parsing/formatting on top of the stdout buffer.
pub const MAX_SEARCH_ENTRIES: usize = 200_000;

/// Default wall-clock budget for a backend search. The contract has no
/// timeout field, so this bounds a runaway scan instead of hanging forever;
/// the `grep` tool has its own, caller-configurable budget.
pub const BACKEND_SEARCH_TIMEOUT_MS: u64 = 300_000;

/// Version-control metadata directories that are never searched.
pub const VCS_DIRECTORIES_TO_EXCLUDE: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// What the search should emit. This is intentionally *not* the public
/// [`GrepMode`]: the backend contract has one "count" flavor while the tool
/// reports `--count-matches`, and the two must not be silently conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrepOutputMode {
    /// One entry per file with at least one match (`-l`).
    FilesWithMatches,
    /// One entry per matching line (`-n`), plus any requested context lines.
    /// Context output uses `path-LINE-text` and `--` separators on the wire;
    /// [`collect_entries`] normalizes both to `path:line:text` entries.
    Content,
    /// One entry per file with at least one matching line, as
    /// `path:line_count` (`--count` / `-c`). Identical on both binaries.
    CountLines,
    /// One entry per file, as `path:match_count` (`--count-matches`).
    /// ripgrep only; the grep fallback degrades to [`Self::CountLines`].
    CountMatches,
}

/// Context lines requested around each match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GrepContext {
    /// No context.
    #[default]
    None,
    /// Symmetric context (`-C`).
    Uniform(u32),
    /// Explicit before/after context (`-B` / `-A`), either side optional.
    Around {
        before: Option<u32>,
        after: Option<u32>,
    },
}

/// Everything needed to build one `rg`/`grep` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepCommandSpec {
    /// Regex source (both binaries are regex engines; grep runs with `-P`).
    pub pattern: String,
    /// Search root as the child process sees it: `"."` for a recursive
    /// directory search, or a single file name relative to the process cwd.
    /// Every target except the recursive `"."` directory is treated as a
    /// single file for the file-name prefix: the builders pass
    /// `--with-filename` / `-H`, so Content/Count entries always start with
    /// `path:`.
    pub target: String,
    /// Whether the search walks a directory tree. Drives `-r`, the VCS
    /// `--exclude-dir` list, and recursive-only glob filtering in the grep
    /// fallback (rg is recursive by default and always applies globs).
    /// Filename prefixing keys off `target` as well, so a single-file target
    /// still gets path-prefixed entries if this flag is set.
    pub recursive: bool,
    /// Output shape.
    pub output: GrepOutputMode,
    /// Raw glob patterns, `!`-prefixed for exclusions (ripgrep syntax).
    /// `rg` gets them verbatim as `--glob`; the grep fallback translates
    /// basename patterns into `--include=` / `--exclude=` / `--exclude-dir=`.
    /// Path-style negations cannot be expressed by those flags and are applied
    /// by [`collect_entries`] instead.
    pub globs: Vec<String>,
    /// Case-insensitive matching (`-i`).
    pub case_insensitive: bool,
    /// Multiline mode (`-U --multiline-dotall`). ripgrep only.
    pub multiline: bool,
    /// ripgrep file type (`--type`). ripgrep only.
    pub file_type: Option<String>,
    /// Emit line numbers in [`GrepOutputMode::Content`] mode (`-n`).
    pub line_numbers: bool,
    /// Context lines (content mode only). Both binaries emit `path-LINE-text`
    /// for them; [`collect_entries`] keeps them and normalizes the entry
    /// shape.
    pub context: GrepContext,
    /// Per-file cap on emitted matches in [`GrepOutputMode::Content`] mode
    /// (`--max-count` / `-m`). Bounds the pathological single-file case; any
    /// *global* cap stays with the caller.
    pub per_file_cap: Option<u32>,
}

impl GrepCommandSpec {
    /// Build the spec shared by the backend implementations of
    /// `OperationSearch::grep`.
    ///
    /// Both backends call this so their argv cannot diverge. `query` is a
    /// regex, `include` is a glob, and the search always targets the base
    /// directory recursively. Line numbers are always on in content mode
    /// because that is the documented entry shape (`path:line:text`).
    pub fn for_backend_grep(request: &GrepRequest) -> Self {
        let output = match request.mode {
            GrepMode::FilesWithMatches => GrepOutputMode::FilesWithMatches,
            GrepMode::Content => GrepOutputMode::Content,
            GrepMode::Count => GrepOutputMode::CountLines,
        };
        // `Some(0)` means "no limit" for this contract (the `grep` tool maps
        // its own 0 to a 2000-entry cap instead), so it must not turn into
        // `--max-count 0`, which matches nothing.
        let per_file_cap = match request.mode {
            GrepMode::Content => request
                .head_limit
                .filter(|limit| *limit > 0)
                .map(|limit| u32::try_from(limit).unwrap_or(u32::MAX)),
            GrepMode::FilesWithMatches | GrepMode::Count => None,
        };
        Self {
            pattern: request.query.clone(),
            target: ".".to_string(),
            recursive: true,
            output,
            globs: request
                .include
                .as_deref()
                .map(parse_globs)
                .unwrap_or_default(),
            case_insensitive: false,
            multiline: false,
            file_type: None,
            line_numbers: true,
            context: GrepContext::None,
            per_file_cap,
        }
    }

    /// Whether the child must be told to prefix matches with the file name.
    ///
    /// Both `rg` and GNU grep print only `line:text` / a bare count when the
    /// target is a single file, even if `-r` is also present. The entry
    /// contract always needs `path:…`, so only the canonical recursive `"."`
    /// search may omit the explicit flag.
    fn needs_filename_prefix(&self) -> bool {
        !(self.recursive && self.target == ".")
    }

    /// ripgrep argv (excluding the program name and the caller's cwd).
    pub fn rg_args(&self) -> Vec<String> {
        let mut args = vec![
            "--hidden".to_string(),
            "--max-columns".to_string(),
            RG_MAX_COLUMNS.to_string(),
        ];

        // Both binaries omit the file name for a single-file target, even
        // with `-r`; force it so Content/Count keep the documented `path:…`
        // shape that `collect_entries` parses (a bare `line:text` / count
        // would be mistaken for a path or dropped).
        if self.needs_filename_prefix() {
            args.push("--with-filename".to_string());
        }

        if self.multiline {
            args.push("-U".to_string());
            args.push("--multiline-dotall".to_string());
        }

        if self.case_insensitive {
            args.push("-i".to_string());
        }

        match self.output {
            GrepOutputMode::FilesWithMatches => args.push("-l".to_string()),
            GrepOutputMode::CountLines => args.push("--count".to_string()),
            GrepOutputMode::CountMatches => args.push("--count-matches".to_string()),
            GrepOutputMode::Content => {
                // `--max-count` is per-file, so a file with millions of
                // matches still stops early. Order matters here only in that
                // it has to match `grep_args`; rg does not care.
                if let Some(cap) = self.per_file_cap {
                    args.push("--max-count".to_string());
                    args.push(cap.to_string());
                }
                if self.line_numbers {
                    args.push("-n".to_string());
                }
                self.push_context(&mut args);
            }
        }

        // `-e` keeps a leading-dash pattern from being parsed as a flag.
        if self.pattern.starts_with('-') {
            args.push("-e".to_string());
            args.push(self.pattern.clone());
        } else {
            args.push(self.pattern.clone());
        }

        if let Some(file_type) = self.file_type.as_ref() {
            args.push("--type".to_string());
            args.push(file_type.clone());
        }

        for glob in &self.globs {
            // Brace alternations are expanded (and trimmed) here as well:
            // `*.{rs, txt}` must not reach ripgrep with a literal leading
            // space in the second alternative. Patterns without braces pass
            // through unchanged.
            let (negated, pattern) = match glob.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, glob.as_str()),
            };
            for pattern in expand_braces(pattern) {
                args.push("--glob".to_string());
                args.push(if negated {
                    format!("!{pattern}")
                } else {
                    pattern
                });
            }
        }

        // ripgrep gives later `--glob` patterns precedence, so the VCS
        // exclusions must come *after* the caller's globs: a positive
        // `--glob '*'` / `'**/*'` would otherwise re-include `.git`.
        for dir in VCS_DIRECTORIES_TO_EXCLUDE {
            args.push("--glob".to_string());
            args.push(format!("!{dir}"));
        }

        args.push(self.target.clone());
        args
    }

    /// GNU `grep` argv (excluding the program name and the caller's cwd).
    ///
    /// Mirrors [`Self::rg_args`] as closely as grep allows: `-P` selects the
    /// PCRE engine (closest to ripgrep's regex flavor), `-I` skips binary
    /// files the way ripgrep does by default, and the VCS directories are
    /// excluded per directory. `--type` and multiline mode have no grep
    /// equivalent and are dropped; degrading to a superset of results is
    /// safer than hard-failing the whole call.
    ///
    /// Negated globs that contain a path (`!src/*.rs`) are deliberately *not*
    /// translated to `--exclude`/`--exclude-dir`: those flags match file names
    /// only, so the basename reduction would exclude far more than the glob
    /// asks for (`!src/*.rs` → `--exclude=*.rs` drops every Rust file).
    /// Such patterns are applied exactly by [`collect_entries`] (including
    /// directory pruning); a caller that consumes the raw stdout without that
    /// post-filter therefore sees a superset of the requested exclusions.
    pub fn grep_args(&self) -> Vec<String> {
        let mut args = vec!["-P".to_string(), "-I".to_string()];
        // Same reason as `rg_args`: a one-file search must still print the
        // file name so the `path:line:text` / `path:count` shape holds.
        if self.needs_filename_prefix() {
            args.push("-H".to_string());
        }

        if self.recursive {
            args.push("-r".to_string());
            for dir in VCS_DIRECTORIES_TO_EXCLUDE {
                args.push(format!("--exclude-dir={dir}"));
            }
        }

        if self.case_insensitive {
            args.push("-i".to_string());
        }

        match self.output {
            GrepOutputMode::FilesWithMatches => args.push("-l".to_string()),
            // grep has no match-count mode; matching lines are the closest
            // (and for `CountLines` the exact) equivalent.
            GrepOutputMode::CountLines | GrepOutputMode::CountMatches => {
                args.push("-c".to_string())
            }
            GrepOutputMode::Content => {
                if let Some(cap) = self.per_file_cap {
                    args.push("-m".to_string());
                    args.push(cap.to_string());
                }
                if self.line_numbers {
                    args.push("-n".to_string());
                }
                self.push_context(&mut args);
            }
        }

        args.push("-e".to_string());
        args.push(self.pattern.clone());

        // `--include`/`--exclude` only make sense for a recursive walk; on a
        // single-file target they would silently change the meaning.
        if self.recursive {
            for glob in &self.globs {
                match glob.strip_prefix('!') {
                    Some(excluded) => {
                        for pattern in expand_braces(excluded) {
                            // grep's `--exclude*` flags match the file name
                            // only. A path-style negation (`!src/*.rs`) has no
                            // basename equivalent: reducing it to `*.rs` would
                            // exclude every Rust file, not just the ones under
                            // `src/`. Skip the prefilter and let
                            // `collect_entries` apply the exact path match.
                            if pattern.contains('/') {
                                continue;
                            }
                            if let Some(pattern) = grep_prefilter(&pattern) {
                                args.push(format!("--exclude={pattern}"));
                                // ripgrep's negative glob prunes matching
                                // directories too (`!node_modules` drops
                                // everything under it); `--exclude` only
                                // matches file names, so mirror the directory
                                // half explicitly.
                                args.push(format!("--exclude-dir={pattern}"));
                            }
                        }
                    }
                    None => {
                        for pattern in expand_braces(glob) {
                            if let Some(pattern) = grep_prefilter(&pattern) {
                                args.push(format!("--include={pattern}"));
                            }
                        }
                    }
                }
            }
        }

        args.push(self.target.clone());
        args
    }

    fn push_context(&self, args: &mut Vec<String>) {
        match self.context {
            GrepContext::None => {}
            GrepContext::Uniform(lines) => {
                args.push("-C".to_string());
                args.push(lines.to_string());
            }
            GrepContext::Around { before, after } => {
                if let Some(before) = before {
                    args.push("-B".to_string());
                    args.push(before.to_string());
                }
                if let Some(after) = after {
                    args.push("-A".to_string());
                    args.push(after.to_string());
                }
            }
        }
    }
}

/// Split a user-supplied glob list into ripgrep-style patterns.
///
/// Accepts `,`- and whitespace-separated lists (`"*.rs, !target/**"`),
/// keeps any `!` negation prefix (ripgrep's own syntax), and leaves braces
/// alone: `"*.{ts,tsx}"` is one pattern, not two, because the comma inside
/// the alternation is part of the glob. The grep fallback expands the
/// alternation itself (see [`expand_braces`]).
pub fn parse_globs(raw: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for ch in raw.chars() {
        match ch {
            '{' => {
                depth += 1;
                current.push(ch);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' | ' ' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    patterns.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        patterns.push(trimmed.to_string());
    }
    patterns
}

/// Expand `{a,b}` alternations into every concrete pattern.
///
/// ripgrep's glob engine understands brace alternation; GNU grep's
/// `--include` / `--exclude` do not, so the fallback has to spell the
/// alternatives out (`*.{ts,tsx}` → `*.ts` + `*.tsx`).
///
/// Nested alternations (`*.{rs,{ts,tsx}}`) and commas inside a nested brace
/// are handled by matching the *outer* closing brace and splitting only on
/// top-level commas; a naive `find('}')` + `split(',')` would emit malformed
/// patterns such as `*.{c`. Unbalanced braces are left untouched so a
/// literal `{` still reaches the binary.
pub fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close) = matching_close_brace(pattern, open) else {
        return vec![pattern.to_string()];
    };
    let head = &pattern[..open];
    let body = &pattern[open + 1..close];
    let tail = &pattern[close + 1..];
    let mut expanded = Vec::new();
    for alternative in split_top_level_alternatives(body) {
        // `*.{rs, txt}` is as common as `*.{rs,txt}`; the space is formatting,
        // not part of the pattern.
        for pattern in expand_braces(&format!("{head}{}{tail}", alternative.trim())) {
            expanded.push(pattern);
        }
    }
    expanded
}

/// Index of the `}` that closes the `{` at `open`, honouring nesting.
fn matching_close_brace(pattern: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in pattern[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a brace body on commas that are not inside a nested brace pair.
fn split_top_level_alternatives(body: &str) -> Vec<&str> {
    let mut alternatives = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                alternatives.push(&body[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    alternatives.push(&body[start..]);
    alternatives
}

/// Compile one glob pattern, mapping a syntax error to `InvalidPath`.
///
/// Shared by the `glob` operation of both backends and by the `include` globs
/// of `grep`, so the error variant and wording cannot drift between them.
pub fn compile_glob(pattern: &str) -> Result<glob::Pattern, OperationError> {
    glob::Pattern::new(pattern).map_err(|error| OperationError::InvalidPath {
        message: format!("invalid glob pattern: {error}"),
    })
}

/// Report the first glob that no pattern engine can parse, if any.
///
/// The argv is handed to `rg` / `grep` as-is, so an unparsable pattern used to
/// surface as a binary exit code (2) and then silently degrade to the other
/// binary, whose `--include` is a literal and matches nothing. Validating up
/// front keeps that from turning into an empty result that looks like "no
/// matches". `!` prefixes and `{a,b}` alternations are stripped/expanded first
/// because that is what actually reaches the binaries.
pub fn invalid_glob(globs: &[String]) -> Option<String> {
    for glob in globs {
        let pattern = glob.strip_prefix('!').unwrap_or(glob.as_str());
        for expanded in expand_braces(pattern) {
            if compile_glob(expanded.as_str()).is_err() {
                return Some(glob.clone());
            }
        }
    }
    None
}

/// Shorten a search binary's stderr for an error message.
///
/// A tree with unreadable directories can produce hundreds of lines; dumping
/// all of them into the tool result burns the caller's context for no gain.
/// The first few lines usually carry the cause.
pub fn stderr_summary(stderr: &[u8]) -> String {
    const MAX_LINES: usize = 4;
    const MAX_CHARS: usize = 400;
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let total_lines = trimmed.lines().count();
    let mut summary = trimmed
        .lines()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let mut truncated = total_lines > MAX_LINES;
    if summary.chars().count() > MAX_CHARS {
        summary = summary.chars().take(MAX_CHARS).collect();
        truncated = true;
    }
    if truncated {
        summary.push_str(&format!(
            "\n… ({total_lines} lines, {} bytes in total)",
            trimmed.len()
        ));
    }
    summary
}

/// The one timeout error shared by every backend implementation.
pub fn timeout_error() -> OperationError {
    OperationError::ExecutionFailed {
        message: format!("search timed out after {BACKEND_SEARCH_TIMEOUT_MS}ms"),
    }
}

/// Strip the `./` / `.\\` prefix a search binary puts on relative paths.
fn strip_dot_prefix(reported: &str) -> &str {
    let stripped = reported.strip_prefix("./").unwrap_or(reported);
    stripped.strip_prefix(".\\").unwrap_or(stripped)
}

/// Reduce a *positive* ripgrep-style glob to the basename form GNU grep
/// understands.
///
/// `grep --include` matches the file name only, while a ripgrep glob may
/// contain a path (`src/*.rs`, `**/*.rs`). Keeping only the last component
/// yields a safe *superset* which [`collect_entries`] narrows with the exact
/// path match. `None` means "no prefilter needed" (`*`, `**`, an empty
/// component).
///
/// This is not valid for a negated pattern: there the basename form is a
/// superset of the *excluded* set, so using it silently drops results. See
/// [`GrepCommandSpec::grep_args`] for how negations are handled instead.
fn grep_prefilter(pattern: &str) -> Option<String> {
    let last = pattern.rsplit('/').next().unwrap_or(pattern).trim();
    if last.is_empty() || last == "*" || last == "**" {
        return None;
    }
    Some(last.to_string())
}

/// A glob list compiled once, for repeated path filtering.
///
/// Mirrors ripgrep's rules: patterns containing `/` match the whole relative
/// path, patterns without one match the file name at any depth, `{a,b}`
/// expands, `!`-prefixed patterns exclude, and a trailing `/` restricts a
/// pattern to directories. **Later patterns win** over earlier ones
/// (ripgrep's `--glob` order): `!src/*.rs, src/*.rs` includes Rust files
/// while the reverse order excludes them.
///
/// A negative pattern that matches a directory prunes its whole subtree (both
/// `rg` and `grep` do this), so a file is also excluded when any ancestor
/// directory's last matching pattern is negative. Patterns that no glob engine
/// can parse are skipped (non-filtering); treating them as "matches" would let
/// a negated unparsable glob drop every entry.
struct GlobFilter {
    patterns: Vec<CompiledGlob>,
    has_positive: bool,
}

/// Options used for every post-filter match.
const GLOB_MATCH_OPTIONS: glob::MatchOptions = glob::MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};

struct CompiledGlob {
    negated: bool,
    /// Patterns containing `/` match the whole relative path; the rest match
    /// the file name at any depth.
    match_whole_path: bool,
    /// A trailing `/` means "directory only": the pattern never matches a file
    /// entry itself, only ancestor directories.
    directory_only: bool,
    pattern: glob::Pattern,
}

impl GlobFilter {
    fn compile(globs: &[String]) -> Self {
        let mut patterns = Vec::new();
        let mut has_positive = false;
        for glob in globs {
            let (negated, raw) = match glob.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, glob.as_str()),
            };
            for expanded in expand_braces(raw) {
                // `!gen/` / `src/` restrict the glob to directories. Strip the
                // trailing separator for matching (ancestor prefixes are
                // tested without it) but remember the restriction so the
                // pattern cannot match a file of the same name.
                let (candidate, directory_only) = match expanded.strip_suffix('/') {
                    Some(stripped) if !stripped.is_empty() => (stripped, true),
                    _ => (expanded.as_str(), false),
                };
                let Ok(pattern) = glob::Pattern::new(candidate) else {
                    continue;
                };
                has_positive |= !negated;
                patterns.push(CompiledGlob {
                    negated,
                    match_whole_path: candidate.contains('/'),
                    directory_only,
                    pattern,
                });
            }
        }
        Self {
            patterns,
            has_positive,
        }
    }

    /// The last matching pattern's decision for `target`, or `None` when no
    /// pattern applies. `is_directory` lets directory-only patterns apply only
    /// to ancestor directories.
    fn decision_for(&self, target: &str, is_directory: bool) -> Option<bool> {
        let mut decision = None;
        for compiled in &self.patterns {
            if compiled.directory_only && !is_directory {
                continue;
            }
            let subject = if compiled.match_whole_path {
                target
            } else {
                target.rsplit('/').next().unwrap_or(target)
            };
            if compiled.pattern.matches_with(subject, GLOB_MATCH_OPTIONS) {
                decision = Some(!compiled.negated);
            }
        }
        decision
    }

    fn matches(&self, relative: &str) -> bool {
        // Emulate the binaries' directory pruning for the exact path filter:
        // a file is excluded when any ancestor directory's last matching
        // pattern is negative.
        let mut search_from = 0;
        while let Some(offset) = relative[search_from..].find('/') {
            let end = search_from + offset;
            if end > 0 && self.decision_for(&relative[..end], true) == Some(false) {
                return false;
            }
            search_from = end + 1;
        }
        self.decision_for(relative, false)
            .unwrap_or(!self.has_positive)
    }
}

/// Whether `relative` (a path relative to the search root) passes the
/// caller's glob list.
///
/// Convenience wrapper around [`GlobFilter`] for one-off checks; call sites
/// that filter many entries compile the list once instead.
pub fn path_matches_globs(relative: &str, globs: &[String]) -> bool {
    GlobFilter::compile(globs).matches(relative)
}

/// Path of `reported` relative to `base_native`, without a `./` prefix.
fn relative_to(base_native: &str, reported: &str) -> String {
    let reported = strip_dot_prefix(reported);
    let base = base_native.trim_end_matches(['/', '\\']);
    if let Some(rest) = reported.strip_prefix(base) {
        if rest.is_empty() {
            return String::new();
        }
        // A plain string prefix is not a path prefix: `/tmp/ws` must not
        // swallow `/tmp/workspace2/x` into `orkspace2/x`.
        if rest.starts_with('/') || rest.starts_with('\\') {
            return rest.trim_start_matches(['/', '\\']).to_string();
        }
    }
    reported.to_string()
}

/// Why a result set exceeds the safe buffering budget, if it does.
fn budget_error(stdout_bytes: usize, entry_count: usize) -> Option<String> {
    if stdout_bytes > MAX_SEARCH_OUTPUT_BYTES {
        return Some(format!(
            "search output too large to buffer safely ({} MiB captured, limit {} MiB): \
             narrow the search root/pattern, add include filters, or lower head_limit",
            stdout_bytes / (1024 * 1024),
            MAX_SEARCH_OUTPUT_BYTES / (1024 * 1024),
        ));
    }
    if entry_count > MAX_SEARCH_ENTRIES {
        return Some(format!(
            "search returned more than {MAX_SEARCH_ENTRIES} results: \
             narrow the search root/pattern or add include filters"
        ));
    }
    None
}

/// Whether a raw stdout line can become a result entry.
///
/// The entry budget must count results, not raw lines: `grep -rc` prints
/// `path:0` for every file it visited (including non-matching ones), and
/// context output contains a bare `--` between groups. Neither is an entry.
/// Counting either would turn a tree with many non-matching files into a
/// bogus "too many results" failure and would also materialise those lines.
///
/// Count rows that cannot be parsed are not candidates either: the count
/// parser below drops them, so they must not consume the budget.
fn is_result_candidate(line: &str, output: GrepOutputMode) -> bool {
    if line.is_empty() {
        return false;
    }
    match output {
        GrepOutputMode::FilesWithMatches => true,
        GrepOutputMode::Content => line != "--" && line != "--\r",
        GrepOutputMode::CountLines | GrepOutputMode::CountMatches => {
            match parse_count_entry(line) {
                Some((_, count)) => count.parse::<u64>().is_ok_and(|count| count != 0),
                None => false,
            }
        }
    }
}

/// Turn one search binary's stdout into contract entries.
///
/// Both backends call this after running the child with its cwd set to the
/// search root, which is what makes their `entries` comparable:
///
/// - [`GrepOutputMode::FilesWithMatches`]: `-l` prints one path per line and
///   filenames may themselves contain `:`, so the whole line is the path;
/// - [`GrepOutputMode::Content`]: `path:line_no:text`. The line number is
///   located as `:<digits>:` rather than at the first `:`, so a path that
///   contains `:` (e.g. `weird:name.txt:12:text`) is not truncated. Context
///   lines (`path-LINE-text`) and the `--` separators between non-contiguous
///   groups, emitted when [`GrepCommandSpec::context`] is set, are parsed as
///   well; context lines are normalized to the same `path:line_no:text`
///   shape, and over-long text is replaced by the placeholder `rg` prints
///   (see [`RG_MAX_COLUMNS`]);
/// - [`GrepOutputMode::CountLines`] / [`GrepOutputMode::CountMatches`]:
///   `path:count`, split from the right because only the path may contain a
///   separator. `grep -c` also emits `path:0` for files without matches while
///   ripgrep does not, so zero counts are dropped.
///
/// The default text format is inherently ambiguous for a path that literally
/// contains `:<digits>:` (indistinguishable from a line-number prefix); the
/// first numbered separator wins. A NUL-separated format would remove that
/// ambiguity, but it would also change what the streaming `grep` tool sees, so
/// it is deliberately out of scope here.
///
/// `limit` is the caller's `head_limit` (`Some(0)` is normalised to `None`
/// before the call). Sorting and filtering run on borrowed slices; only the
/// entries that survive `limit` are formatted, so a small limit on a large
/// result set does not materialise the whole output twice.
///
/// Fails when the captured stdout exceeds [`MAX_SEARCH_OUTPUT_BYTES`] or the
/// number of candidate result lines exceeds [`MAX_SEARCH_ENTRIES`].
pub fn collect_entries(
    stdout: &str,
    base_native: &str,
    output: GrepOutputMode,
    globs: &[String],
    limit: Option<usize>,
) -> Result<Vec<String>, OperationError> {
    // The grep fallback can only prefilter on basenames, so a path-style glob
    // needs a second, exact pass here. Basename-only globs are already exact
    // on both binaries and skip it, keeping the common path untouched.
    // Compile the list once per search instead of once per entry.
    let filter = globs
        .iter()
        .any(|glob| glob.trim_start_matches('!').contains('/'))
        .then(|| GlobFilter::compile(globs));
    // Count candidate result lines before materialising `Vec<&str>` so the
    // entry ceiling bounds both the checks and that vector's allocation. Raw
    // lines are not results (see `is_result_candidate`), so a large tree of
    // non-matching files must not trip the entry budget.
    let line_count = stdout
        .lines()
        .filter(|line| is_result_candidate(line, output))
        .count();
    if let Some(message) = budget_error(stdout.len(), line_count) {
        return Err(OperationError::ExecutionFailed { message });
    }
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| is_result_candidate(line, output))
        .collect();
    let keep = limit.unwrap_or(usize::MAX);

    // Sorting by the reported (relative) path is equivalent to sorting by the
    // resolved absolute path: every entry shares the same search root prefix.
    let entries = match output {
        GrepOutputMode::FilesWithMatches => {
            let mut paths: Vec<&str> = lines
                .into_iter()
                .filter(|line| {
                    filter
                        .as_ref()
                        .is_none_or(|filter| filter.matches(&relative_to(base_native, line)))
                })
                .collect();
            paths.sort_unstable_by(|a, b| strip_dot_prefix(a).cmp(strip_dot_prefix(b)));
            paths.dedup_by(|a, b| strip_dot_prefix(a) == strip_dot_prefix(b));
            paths
                .into_iter()
                .take(keep)
                .map(|reported| resolve_reported_path(base_native, reported))
                .collect()
        }
        GrepOutputMode::Content => {
            let mut rows: Vec<ContentLine<'_>> = lines
                .into_iter()
                .filter_map(|line| {
                    let entry = parse_content_line(line)?;
                    if let Some(filter) = filter.as_ref() {
                        if !filter.matches(&relative_to(base_native, entry.reported)) {
                            return None;
                        }
                    }
                    Some(entry)
                })
                .collect();
            rows.sort_unstable_by(|a, b| {
                strip_dot_prefix(a.reported)
                    .cmp(strip_dot_prefix(b.reported))
                    .then(a.line_number.cmp(&b.line_number))
                    .then(a.text.cmp(b.text))
            });
            rows.into_iter()
                .take(keep)
                .map(|entry| {
                    let path = resolve_reported_path(base_native, entry.reported);
                    let text = bounded_content_text(entry.text, entry.is_context);
                    match entry.line_number {
                        Some(number) => format!("{path}:{number}:{text}"),
                        None => format!("{path}:{text}"),
                    }
                })
                .collect()
        }
        GrepOutputMode::CountLines | GrepOutputMode::CountMatches => {
            let mut rows: Vec<(&str, &str)> = lines
                .into_iter()
                .filter_map(|line| {
                    let (reported, count) = parse_count_entry(line)?;
                    if let Some(filter) = filter.as_ref() {
                        if !filter.matches(&relative_to(base_native, reported)) {
                            return None;
                        }
                    }
                    if count.parse::<u64>().ok()? == 0 {
                        return None;
                    }
                    Some((reported, count))
                })
                .collect();
            rows.sort_unstable_by(|a, b| strip_dot_prefix(a.0).cmp(strip_dot_prefix(b.0)));
            rows.into_iter()
                .take(keep)
                .map(|(reported, count)| {
                    format!("{}:{count}", resolve_reported_path(base_native, reported))
                })
                .collect()
        }
    };
    Ok(entries)
}

/// One parsed `Content` output line.
///
/// `reported` and `text` borrow from the captured stdout so sorting and
/// truncation do not allocate for entries that are eventually dropped.
struct ContentLine<'a> {
    reported: &'a str,
    line_number: Option<u64>,
    text: &'a str,
    /// Whether the binary marked this as a context line (`path-LINE-text`)
    /// rather than a match (`path:LINE:text`).
    is_context: bool,
}

/// Parse one `Content` output line from `rg` or GNU grep.
///
/// With line numbers enabled the binaries print `path:LINE:text` for matches
/// and `path-LINE-text` for context lines, with a bare `--` between
/// non-contiguous groups. The separator cannot be found by taking the first
/// `:` because file names may contain `:`; instead the line number is located
/// as `:<digits>:` (then `-<digits>-` for context). Without a numbered
/// separator the line falls back to the old `path:payload` split, which keeps
/// no-line-number output working (that shape is inherently ambiguous).
fn parse_content_line(line: &str) -> Option<ContentLine<'_>> {
    if line == "--" || line == "--\r" {
        return None;
    }
    if let Some((reported, line_number, text)) = split_numbered_entry(line, ':') {
        return Some(ContentLine {
            reported,
            line_number: Some(line_number),
            text,
            is_context: false,
        });
    }
    if let Some((reported, line_number, text)) = split_numbered_entry(line, '-') {
        return Some(ContentLine {
            reported,
            line_number: Some(line_number),
            text,
            is_context: true,
        });
    }
    let (reported, text) = split_entry(line)?;
    Some(ContentLine {
        reported,
        line_number: None,
        text,
        is_context: false,
    })
}

/// Split a line at the first `<separator><digits><separator>` sequence.
///
/// Returns the path before the sequence, the parsed line number, and the text
/// after it. Scanning stops at the first candidate that parses as a `u64`; a
/// digit run too large for `u64` is not treated as a separator.
fn split_numbered_entry(line: &str, separator: char) -> Option<(&str, u64, &str)> {
    let mut search_from = 0;
    while let Some(offset) = line[search_from..].find(separator) {
        let start = search_from + offset;
        let digits_start = start + separator.len_utf8();
        let mut digits_end = digits_start;
        let bytes = line.as_bytes();
        while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
            digits_end += 1;
        }
        if digits_end > digits_start && line[digits_end..].starts_with(separator) {
            if let Ok(number) = line[digits_start..digits_end].parse::<u64>() {
                return Some((
                    &line[..start],
                    number,
                    &line[digits_end + separator.len_utf8()..],
                ));
            }
        }
        search_from = digits_start;
    }
    None
}

/// Replace over-long content text with the placeholder `rg` would print.
///
/// `rg --max-columns` already does this; GNU grep has no equivalent flag, so
/// this is the fallback's only line-length protection. Replacing the whole
/// text (rather than slicing it) also keeps UTF-8 boundaries intact.
fn bounded_content_text(text: &str, is_context: bool) -> &str {
    if text.len() <= RG_MAX_COLUMNS as usize {
        text
    } else if is_context {
        OMITTED_LONG_CONTEXT_LINE
    } else {
        OMITTED_LONG_MATCH_LINE
    }
}

/// Whether an exit code means "the search ran" (0 = matches, 1 = no
/// matches). Anything else is a real error, including the 127 a shell
/// reports when neither binary exists.
pub fn exit_code_is_success(exit_code: Option<i32>) -> bool {
    matches!(exit_code, Some(0) | Some(1))
}

/// Split a `path:count` line into its path and (trimmed) count.
///
/// Returns `None` when the line has no separator; callers decide whether that
/// is a reason to keep or drop it.
fn parse_count_entry(entry: &str) -> Option<(&str, &str)> {
    let (reported, count) = entry.rsplit_once(':')?;
    Some((reported, count.trim()))
}

/// Drop `path:0` entries.
///
/// `grep -rc` emits a line for every file it visited (`path:0` included);
/// ripgrep's `--count` only emits files with matches. Filtering keeps the
/// two binaries' output identical.
pub fn drop_zero_count_entries(entries: Vec<String>) -> Vec<String> {
    entries
        .into_iter()
        .filter(|entry| match parse_count_entry(entry) {
            // Unparsable counts are kept: callers only promise to drop `:0`.
            Some((_, count)) => count.parse::<u64>().ok().map_or(true, |count| count != 0),
            None => true,
        })
        .collect()
}

/// Split a `path:payload` line into its reported path and remainder.
///
/// This is the plain first-`:` split, kept for callers that only need the
/// `path:payload` shape (`count` lines use [`parse_count_entry`], `content`
/// lines use [`parse_content_line`]). The reported path is relative to the
/// search root (`./src/main.rs`) whenever the child ran with that cwd.
/// Returns `None` for lines without a separator (stray diagnostics).
pub fn split_entry(line: &str) -> Option<(&str, &str)> {
    line.split_once(':')
}

/// Turn a path reported by the child process into a backend-native path.
///
/// `reported` is relative to the search root (`./a/b.rs`, `.\\a\\b.rs`) or
/// already absolute; either way the result is anchored at `base_native`, so
/// both backends return the same absolute paths.
pub fn resolve_reported_path(base_native: &str, reported: &str) -> String {
    let stripped = strip_dot_prefix(reported);
    if stripped.is_empty() || stripped == "." {
        return base_native.to_string();
    }
    let path = Path::new(stripped);
    if path.is_absolute() {
        return stripped.to_string();
    }
    Path::new(base_native)
        .join(path)
        .to_string_lossy()
        .into_owned()
}
