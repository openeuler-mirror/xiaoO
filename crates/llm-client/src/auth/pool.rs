use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use super::state::AuthState;

pub type CredentialId = String;
pub type ProviderId = String;

#[derive(Clone, Debug)]
pub struct AuthCredential {
    pub id: CredentialId,
    pub provider: ProviderId,
    pub api_key: String,
    pub priority: u32,
    pub state: AuthState,
    pub metadata: HashMap<String, String>,
}

impl AuthCredential {
    pub fn new(
        id: impl Into<String>,
        provider: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            api_key: api_key.into(),
            priority: 100,
            state: AuthState::ready(),
            metadata: HashMap::new(),
        }
    }

    pub fn with_priority(mut self, priority: u32) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_state(mut self, state: AuthState) -> Self {
        self.state = state;
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn is_available(&self) -> bool {
        self.state.is_available()
    }
}

pub trait AuthPool: Send + Sync {
    fn get_available(&self, provider: &str, model: &str) -> Option<AuthCredential>;
    fn mark_cooldown(&self, id: &str, duration: Duration);
    fn mark_blocked(&self, id: &str, reason: &str);
    fn refresh(&self, id: &str);
    fn add(&self, credential: AuthCredential);
    fn remove(&self, id: &str);
    fn get(&self, id: &str) -> Option<AuthCredential>;
    fn list(&self) -> Vec<AuthCredential>;
}

pub struct InMemoryAuthPool {
    credentials: RwLock<HashMap<CredentialId, AuthCredential>>,
}

impl Default for InMemoryAuthPool {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryAuthPool {
    pub fn new() -> Self {
        Self {
            credentials: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_credentials(credentials: Vec<AuthCredential>) -> Self {
        let map: HashMap<CredentialId, AuthCredential> =
            credentials.into_iter().map(|c| (c.id.clone(), c)).collect();
        Self {
            credentials: RwLock::new(map),
        }
    }
}

impl AuthPool for InMemoryAuthPool {
    fn get_available(&self, provider: &str, _model: &str) -> Option<AuthCredential> {
        let creds = self.credentials.read().ok()?;
        creds
            .values()
            .filter(|c| c.provider == provider && c.is_available())
            .min_by_key(|c| c.priority)
            .cloned()
    }

    fn mark_cooldown(&self, id: &str, duration: Duration) {
        if let Ok(mut creds) = self.credentials.write() {
            if let Some(cred) = creds.get_mut(id) {
                cred.state = AuthState::cooldown(duration);
            }
        }
    }

    fn mark_blocked(&self, id: &str, reason: &str) {
        if let Ok(mut creds) = self.credentials.write() {
            if let Some(cred) = creds.get_mut(id) {
                cred.state = AuthState::blocked(reason);
            }
        }
    }

    fn refresh(&self, id: &str) {
        if let Ok(mut creds) = self.credentials.write() {
            if let Some(cred) = creds.get_mut(id) {
                cred.state.maybe_recover();
            }
        }
    }

    fn add(&self, credential: AuthCredential) {
        if let Ok(mut creds) = self.credentials.write() {
            creds.insert(credential.id.clone(), credential);
        }
    }

    fn remove(&self, id: &str) {
        if let Ok(mut creds) = self.credentials.write() {
            creds.remove(id);
        }
    }

    fn get(&self, id: &str) -> Option<AuthCredential> {
        let creds = self.credentials.read().ok()?;
        creds.get(id).cloned()
    }

    fn list(&self) -> Vec<AuthCredential> {
        self.credentials
            .read()
            .map(|c| c.values().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/llm-client/auth/pool_test.rs"]
mod tests;
