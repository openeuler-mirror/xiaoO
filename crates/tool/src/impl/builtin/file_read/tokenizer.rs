/// Chars per token ratio for common file extensions.
/// Based on typical LLM tokenization patterns where:
/// - Dense code (Rust, Go): ~3 chars/token
/// - Standard code (Python, JS, Java): ~4 chars/token
/// - Verbose formats (JSON): ~5 chars/token
fn chars_per_token_for_extension(extension: &str) -> f64 {
    static CHARS_PER_TOKEN: &[(&str, f64)] = &[
        // Dense compiled languages
        ("rs", 3.0),
        ("go", 3.0),
        ("c", 3.0),
        ("cpp", 3.0),
        ("h", 3.0),
        ("hpp", 3.0),
        ("cs", 3.5),
        // Standard languages
        ("py", 4.0),
        ("js", 4.0),
        ("ts", 4.0),
        ("jsx", 4.0),
        ("tsx", 4.0),
        ("java", 4.0),
        ("rb", 4.0),
        ("php", 4.0),
        ("swift", 4.0),
        ("kt", 4.0),
        ("scala", 4.0),
        // Web formats
        ("html", 4.0),
        ("css", 4.0),
        ("scss", 4.0),
        ("sass", 4.0),
        ("less", 4.0),
        ("vue", 4.0),
        ("svelte", 4.0),
        // Shell/Scripts
        ("sh", 4.0),
        ("bash", 4.0),
        ("zsh", 4.0),
        ("fish", 4.0),
        ("ps1", 4.0),
        // Data/Config formats
        ("json", 5.0),
        ("xml", 4.5),
        ("yaml", 4.5),
        ("yml", 4.5),
        ("toml", 4.5),
        ("ini", 4.0),
        ("cfg", 4.0),
        ("conf", 4.0),
        // Markup/Documentation
        ("md", 4.0),
        ("rst", 4.0),
        ("tex", 4.0),
        ("txt", 4.0),
        // SQL
        ("sql", 4.0),
        // Docker/DevOps
        ("dockerfile", 4.0),
        ("dockerignore", 4.0),
        ("tf", 4.0),
        ("tfvars", 4.0),
        // Build files
        ("cmake", 4.0),
        ("makefile", 4.0),
        // Other
        ("lua", 4.0),
        ("r", 4.0),
        ("R", 4.0),
        ("pl", 4.0),
        ("pm", 4.0),
        ("hs", 3.5),
        ("erl", 3.5),
        ("ex", 4.0),
        ("exs", 4.0),
        ("fs", 4.0),
        ("fsx", 4.0),
        ("ml", 3.5),
        ("mli", 3.5),
    ];

    for (ext, ratio) in CHARS_PER_TOKEN {
        if extension.eq_ignore_ascii_case(ext) {
            return *ratio;
        }
    }
    4.0 // Default ratio
}

/// Extract file extension from a file path.
fn get_extension(file_path: &str) -> Option<&str> {
    file_path.rsplit('.').next().and_then(|ext| {
        // Handle files like ".gitignore" or "Dockerfile" (no extension)
        if ext.contains('/') || ext.is_empty() {
            None
        } else {
            Some(ext)
        }
    })
}

/// Estimate the number of tokens in a file based on its content and file path.
///
/// Uses file extension to determine appropriate chars-per-token ratio,
/// then divides total character count by that ratio.
pub fn estimate_tokens(file_path: &str, content: &str) -> usize {
    let ratio = get_extension(file_path)
        .map(chars_per_token_for_extension)
        .unwrap_or(4.0);

    let char_count = content.chars().count();
    (char_count as f64 / ratio).ceil() as usize
}
