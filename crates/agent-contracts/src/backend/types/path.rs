use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::time::SystemTime;

/// The filesystem namespace a `BackendPath` refers to inside a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathNamespace {
    /// Under the backend's workspace root.
    Workspace,
    /// Under the backend's home directory.
    Home,
    /// Under the backend's temporary directory.
    Temp,
    /// A path that has not been attributed to a namespace (legacy / raw).
    Uncategorized,
}

impl PathNamespace {
    pub fn as_str(self) -> &'static str {
        match self {
            PathNamespace::Workspace => "workspace",
            PathNamespace::Home => "home",
            PathNamespace::Temp => "temp",
            PathNamespace::Uncategorized => "uncategorized",
        }
    }
}

/// A path in the backend's filesystem namespace.
///
/// Unlike the old `String` newtype, a `BackendPath` carries ownership and
/// namespace metadata so a path produced by one backend cannot silently be
/// interpreted as a path inside a different backend. `native_path` holds the
/// canonical absolute path *inside* that backend (the same value that was
/// previously stored in the tuple field), so existing `Display` / tool output
/// stays byte-for-byte compatible.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BackendPath {
    pub backend_id: String,
    pub namespace: PathNamespace,
    /// Path relative to the namespace root (`""` when `/`).
    pub relative_path: String,
    pub(crate) native_path: String,
}

impl BackendPath {
    /// Build a fully attributed backend path.
    pub fn new(
        backend_id: impl Into<String>,
        namespace: PathNamespace,
        native_path: impl Into<String>,
        relative_path: impl Into<String>,
    ) -> Self {
        Self {
            backend_id: backend_id.into(),
            namespace,
            native_path: native_path.into(),
            relative_path: relative_path.into(),
        }
    }

    /// Build an unattributed path from a raw absolute string. Used for
    /// legacy call sites and tests; backends re-attribute through their
    /// path resolver before operating on it.
    pub fn from_raw(path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            backend_id: String::new(),
            namespace: PathNamespace::Uncategorized,
            relative_path: String::new(),
            native_path: path,
        }
    }

    /// The canonical absolute path inside the owning backend.
    pub fn native(&self) -> &str {
        self.native_path.as_str()
    }

    /// Rebuild this path under the same namespace with a new native path.
    pub fn with_native(
        &self,
        native_path: impl Into<String>,
        relative_path: impl Into<String>,
    ) -> Self {
        Self {
            backend_id: self.backend_id.clone(),
            namespace: self.namespace,
            native_path: native_path.into(),
            relative_path: relative_path.into(),
        }
    }

    /// Rebuild this path with full attribution (used when a resolver classifies a raw path).
    pub fn with_attribution(
        &self,
        backend_id: impl Into<String>,
        namespace: PathNamespace,
        native_path: impl Into<String>,
        relative_path: impl Into<String>,
    ) -> Self {
        Self::new(backend_id, namespace, native_path, relative_path)
    }
}

impl Default for BackendPath {
    fn default() -> Self {
        Self::from_raw(String::new())
    }
}

impl fmt::Display for BackendPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.native_path.fmt(f)
    }
}

impl Serialize for BackendPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Unattributed paths (created via `from_raw`) keep the legacy
        // wire format so old readers see a plain string and round-trip
        // stays symmetric. Fully attributed paths serialize as an object
        // carrying the namespace metadata.
        if self.backend_id.is_empty() {
            serializer.serialize_str(&self.native_path)
        } else {
            #[derive(Serialize)]
            struct Repr<'a> {
                backend_id: &'a str,
                namespace: PathNamespace,
                relative_path: &'a str,
                native_path: &'a str,
            }
            let repr = Repr {
                backend_id: &self.backend_id,
                namespace: self.namespace,
                relative_path: &self.relative_path,
                native_path: &self.native_path,
            };
            repr.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for BackendPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Struct {
                backend_id: String,
                namespace: PathNamespace,
                relative_path: String,
                native_path: String,
            },
            Legacy(String),
        }
        match Repr::deserialize(deserializer)? {
            Repr::Struct {
                backend_id,
                namespace,
                relative_path,
                native_path,
            } => Ok(Self {
                backend_id,
                namespace,
                relative_path,
                native_path,
            }),
            Repr::Legacy(raw) => Ok(Self::from_raw(raw)),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_string_deserializes_to_unattributed_path() {
        let path: BackendPath = serde_json::from_str(r#""/sandbox/tmp/bash-output-1""#).unwrap();
        assert_eq!(path, BackendPath::from_raw("/sandbox/tmp/bash-output-1"));
        assert!(path.backend_id.is_empty());
        assert_eq!(path.namespace, PathNamespace::Uncategorized);
        assert_eq!(path.native(), "/sandbox/tmp/bash-output-1");
    }

    #[test]
    fn unattributed_path_serializes_as_legacy_string() {
        let path = BackendPath::from_raw("/sandbox/tmp/bash-output-1");
        let json = serde_json::to_string(&path).unwrap();
        assert_eq!(json, r#""/sandbox/tmp/bash-output-1""#);
        let back: BackendPath = serde_json::from_str(&json).unwrap();
        assert_eq!(back, path);
        // Display stays byte-for-byte compatible with the old String newtype.
        assert_eq!(path.to_string(), "/sandbox/tmp/bash-output-1");
    }

    #[test]
    fn attributed_path_serializes_as_object_and_round_trips() {
        let path = BackendPath::new(
            "sess_01J_test_attrs",
            PathNamespace::Temp,
            "/tmp/xiaoo-bash-output-abc",
            "xiaoo-bash-output-abc",
        );
        let json = serde_json::to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["backend_id"], "sess_01J_test_attrs");
        assert_eq!(value["namespace"], "temp");
        assert_eq!(value["relative_path"], "xiaoo-bash-output-abc");
        assert_eq!(value["native_path"], "/tmp/xiaoo-bash-output-abc");
        let back: BackendPath = serde_json::from_str(&json).unwrap();
        assert_eq!(back, path);
    }
}
