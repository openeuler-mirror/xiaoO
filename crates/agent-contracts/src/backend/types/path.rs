use std::fmt;
use std::time::SystemTime;

/// A path in the backend's filesystem namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendPath(pub String);

impl fmt::Display for BackendPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The kind of a filesystem path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// Metadata about a filesystem path.
#[derive(Debug, Clone)]
pub struct PathStat {
    pub exists: bool,
    pub kind: Option<PathKind>,
    pub size_bytes: Option<u64>,
    pub modified_at: Option<SystemTime>,
}
