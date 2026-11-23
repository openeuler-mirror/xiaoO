//! Shared constants for FileReadTool.

/// Maximum file read output tokens.
pub const DEFAULT_MAX_TOKENS: usize = 25_000;

/// Maximum file size in bytes.
pub const DEFAULT_MAX_SIZE_BYTES: u64 = 256 * 1024;

/// Default maximum notebook file size in bytes.
pub const DEFAULT_MAX_NOTEBOOK_SIZE: u64 = 10 * 1024 * 1024;

/// Default token budget for image reads before compression.
pub const DEFAULT_IMAGE_MAX_TOKENS: usize = 4096;

/// Estimated bytes per token for base64-encoded images.
pub const IMAGE_BYTES_PER_TOKEN_ESTIMATE: f64 = 3.0;

/// Maximum number of pages allowed per PDF read request.
pub const PDF_MAX_PAGES_PER_READ: u32 = 100;

/// Image extensions supported for reading.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// Image file suffixes allowed during validation.
pub const IMAGE_PATH_SUFFIXES: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg",
];

/// Binary file extensions that are explicitly rejected.
pub const REJECTED_BINARY_EXTENSIONS: &[&str] = &[
    ".exe", ".dll", ".so", ".bin", ".dat", ".pak", ".asset", ".unity", ".wasm",
];

/// Text-like file suffixes allowed during binary detection.
///
/// `is_binary_file_extension` falls back to a heuristic: an extension that is
/// neither here nor an image/PDF suffix is treated as binary when it is short.
/// The list therefore has to cover the text formats an agent is expected to
/// read, otherwise reading e.g. a Ruby or Kotlin source file is rejected with
/// `Binary file rejected`.
///
/// Grouped as: plain text/markup/data, build and config, shell, and
/// programming languages.
pub const TEXT_FILE_EXTENSIONS: &[&str] = &[
    ".txt",
    ".md",
    ".rst",
    ".adoc",
    ".org",
    ".tex",
    ".bib",
    ".json",
    ".jsonl",
    ".ndjson",
    ".xml",
    ".html",
    ".css",
    ".scss",
    ".sass",
    ".less",
    ".csv",
    ".tsv",
    ".diff",
    ".patch",
    ".log",
    ".yaml",
    ".yml",
    ".toml",
    ".ini",
    ".cfg",
    ".conf",
    ".properties",
    ".lock",
    ".env",
    ".plist",
    ".gradle",
    ".cmake",
    ".mk",
    ".make",
    ".hcl",
    ".tf",
    ".tfvars",
    ".proto",
    ".graphql",
    ".gql",
    ".sh",
    ".bash",
    ".zsh",
    ".fish",
    ".ps1",
    ".bat",
    ".cmd",
    ".rs",
    ".py",
    ".ipynb",
    ".go",
    ".java",
    ".kt",
    ".kts",
    ".scala",
    ".groovy",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".cc",
    ".hh",
    ".hxx",
    ".cs",
    ".fs",
    ".fsx",
    ".vb",
    ".m",
    ".mm",
    ".swift",
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".mjs",
    ".cjs",
    ".vue",
    ".svelte",
    ".dart",
    ".php",
    ".rb",
    ".pl",
    ".pm",
    ".lua",
    ".clj",
    ".cljs",
    ".ex",
    ".exs",
    ".erl",
    ".hrl",
    ".hs",
    ".ml",
    ".mli",
    ".zig",
    ".nim",
    ".jl",
    ".r",
    ".vim",
    ".el",
    ".asm",
    ".s",
    ".v",
    ".sv",
    ".vhd",
    ".sql",
];

/// PDF extension without leading dot.
pub const PDF_EXTENSION: &str = "pdf";

/// PDF file suffix with leading dot.
pub const PDF_PATH_SUFFIX: &str = ".pdf";

/// Notebook extension without leading dot.
pub const NOTEBOOK_EXTENSION: &str = "ipynb";

/// Set of blocked device paths.
pub const BLOCKED_DEVICE_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/full",
    "/dev/stdin",
    "/dev/stdout",
    "/dev/stderr",
    "/dev/tty",
    "/dev/console",
    "/dev/fd/0",
    "/dev/fd/1",
    "/dev/fd/2",
];

/// Process-scoped files under `/proc/<pid>/` that must never be readable.
///
/// Unlike the device paths above these do not block — they leak. `environ`
/// carries the daemon's API keys and database credentials, `cmdline` often
/// carries them as `--api-key=...`, `mem` allows reading *and* writing another
/// process's memory, and the layout family (`maps`, `smaps`,
/// `smaps_rollup`, `numa_maps`, `auxv`) exposes where that memory lives and,
/// via `auxv`, the stack canary and `AT_RANDOM` bytes. Blocking `mem` without
/// blocking the layout family would leave the first half of the injection
/// chain wide open. Matched by file name so `/proc/self/...`, `/proc/1/...`
/// and `/proc/<pid>/task/<tid>/...` are all covered.
pub const BLOCKED_PROC_FILES: &[&str] = &[
    "environ",
    "cmdline",
    "mem",
    "maps",
    "smaps",
    "smaps_rollup",
    "numa_maps",
    "auxv",
];
