//! Owns checkpoint resources for the TUI's current local session.
use super::{CheckpointConfig, CheckpointDriver, CheckpointError, UnixCheckpointEndpoint};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct UnixCheckpointSession {
    current: Mutex<CurrentSession>,
}
#[derive(Default)]
struct CurrentSession {
    // Tracks the current target even if binding fails, so frame refreshes do
    // not repeatedly retry a failed socket. A session/mode/config change retries.
    session_id: Option<String>,
    config: Option<CheckpointConfig>,
    endpoint: Option<UnixCheckpointEndpoint>,
}
impl UnixCheckpointSession {
    /// Prepare a local session, or close it for remote mode (`None`). Returns
    /// the new socket path only when an endpoint is actually created.
    pub async fn prepare(
        &self,
        session_id: Option<&str>,
        config: &CheckpointConfig,
    ) -> Result<Option<PathBuf>, CheckpointError> {
        let session_id = session_id.filter(|_| config.enabled);
        let mut current = self.current.lock().await;
        if current.session_id.as_deref() == session_id && current.config.as_ref() == Some(config) {
            return Ok(None);
        }
        if let Some(endpoint) = current.endpoint.take() {
            endpoint.close().await;
        }
        current.session_id = session_id.map(str::to_owned);
        current.config = Some(config.clone());
        if let Some(session_id) = session_id {
            let endpoint =
                UnixCheckpointEndpoint::bind(session_id.to_owned(), config.clone()).await?;
            let path = endpoint.path().to_owned();
            tracing::info!(session_id, path = %path.display(), "context checkpoint socket ready");
            current.endpoint = Some(endpoint);
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }

    pub async fn driver(&self, session_id: &str) -> Option<Arc<CheckpointDriver>> {
        let current = self.current.lock().await;
        if current.session_id.as_deref() != Some(session_id) {
            return None;
        }
        current
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.driver.clone())
    }

    /// Close one session, or every checkpoint resource when no id is supplied.
    pub async fn close(&self, session_id: Option<&str>) {
        let mut current = self.current.lock().await;
        if session_id.is_some() && current.session_id.as_deref() != session_id {
            return;
        }
        current.session_id = None;
        current.config = None;
        if let Some(endpoint) = current.endpoint.take() {
            endpoint.close().await;
        }
    }
}
