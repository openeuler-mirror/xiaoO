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
pub const TEXT_FILE_EXTENSIONS: &[&str] = &[
    ".txt", ".md", ".json", ".xml", ".html", ".css", ".js", ".ts", ".rs", ".py", ".go", ".java",
    ".c", ".cpp", ".h", ".hpp", ".yaml", ".yml", ".toml", ".ini", ".cfg", ".conf", ".log", ".sh",
    ".bash", ".zsh", ".fish", ".ps1", ".bat", ".cmd", ".sql", ".csv", ".tsv",
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
