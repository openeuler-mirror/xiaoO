use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    #[error("failed to read skill file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse SKILL.toml at {path}: {message}")]
    TomlParse { path: PathBuf, message: String },

    #[error("failed to parse SKILL.md frontmatter at {path}: {message}")]
    FrontmatterParse { path: PathBuf, message: String },

    #[error("missing required field '{field}' in {path}")]
    MissingField { path: PathBuf, field: String },
}
