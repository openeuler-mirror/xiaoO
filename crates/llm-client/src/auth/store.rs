use std::collections::HashMap;
use std::sync::RwLock;

use super::pool::AuthCredential;

#[derive(Debug, Clone)]
pub enum AuthStoreError {
    LoadError(String),
    SaveError(String),
    UpdateError(String),
    DeleteError(String),
    NotFound(String),
}

impl std::fmt::Display for AuthStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoadError(msg) => write!(f, "Failed to load credentials: {}", msg),
            Self::SaveError(msg) => write!(f, "Failed to save credentials: {}", msg),
            Self::UpdateError(msg) => write!(f, "Failed to update credential: {}", msg),
            Self::DeleteError(msg) => write!(f, "Failed to delete credential: {}", msg),
            Self::NotFound(id) => write!(f, "Credential not found: {}", id),
        }
    }
}

impl std::error::Error for AuthStoreError {}

pub trait AuthStore: Send + Sync {
    fn load(&self) -> Result<Vec<AuthCredential>, AuthStoreError>;
    fn save(&self, credentials: &[AuthCredential]) -> Result<(), AuthStoreError>;
    fn update(&self, credential: &AuthCredential) -> Result<(), AuthStoreError>;
    fn delete(&self, id: &str) -> Result<(), AuthStoreError>;
}

pub struct InMemoryAuthStore {
    credentials: RwLock<HashMap<String, AuthCredential>>,
}

impl Default for InMemoryAuthStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAuthStore {
    pub fn new() -> Self {
        Self {
            credentials: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_credentials(credentials: Vec<AuthCredential>) -> Self {
        let map: HashMap<String, AuthCredential> =
            credentials.into_iter().map(|c| (c.id.clone(), c)).collect();
        Self {
            credentials: RwLock::new(map),
        }
    }
}

impl AuthStore for InMemoryAuthStore {
    fn load(&self) -> Result<Vec<AuthCredential>, AuthStoreError> {
        let creds = self
            .credentials
            .read()
            .map_err(|e| AuthStoreError::LoadError(e.to_string()))?;
        Ok(creds.values().cloned().collect())
    }

    fn save(&self, credentials: &[AuthCredential]) -> Result<(), AuthStoreError> {
        let mut creds = self
            .credentials
            .write()
            .map_err(|e| AuthStoreError::SaveError(e.to_string()))?;
        creds.clear();
        for cred in credentials {
            creds.insert(cred.id.clone(), cred.clone());
        }
        Ok(())
    }

    fn update(&self, credential: &AuthCredential) -> Result<(), AuthStoreError> {
        let mut creds = self
            .credentials
            .write()
            .map_err(|e| AuthStoreError::UpdateError(e.to_string()))?;
        if !creds.contains_key(&credential.id) {
            return Err(AuthStoreError::NotFound(credential.id.clone()));
        }
        creds.insert(credential.id.clone(), credential.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), AuthStoreError> {
        let mut creds = self
            .credentials
            .write()
            .map_err(|e| AuthStoreError::DeleteError(e.to_string()))?;
        creds
            .remove(id)
            .ok_or_else(|| AuthStoreError::NotFound(id.to_string()))?;
        Ok(())
    }
}

pub struct FileAuthStore {
    _path: std::path::PathBuf,
}

impl FileAuthStore {
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        Self {
            _path: path.as_ref().to_path_buf(),
        }
    }
}

impl AuthStore for FileAuthStore {
    fn load(&self) -> Result<Vec<AuthCredential>, AuthStoreError> {
        Err(AuthStoreError::LoadError(
            "FileAuthStore not yet implemented".to_string(),
        ))
    }

    fn save(&self, _credentials: &[AuthCredential]) -> Result<(), AuthStoreError> {
        Err(AuthStoreError::SaveError(
            "FileAuthStore not yet implemented".to_string(),
        ))
    }

    fn update(&self, _credential: &AuthCredential) -> Result<(), AuthStoreError> {
        Err(AuthStoreError::UpdateError(
            "FileAuthStore not yet implemented".to_string(),
        ))
    }

    fn delete(&self, _id: &str) -> Result<(), AuthStoreError> {
        Err(AuthStoreError::DeleteError(
            "FileAuthStore not yet implemented".to_string(),
        ))
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/auth/store_test.rs"]
mod tests;
