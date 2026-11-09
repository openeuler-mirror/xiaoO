//! Local, newline-delimited JSON transport. One request/response per connection.
//!
//! When enabled, a local TUI opens its control endpoint before accepting task
//! input, at the configured `checkpoint.unix_socket.endpoint` path.
//! The startup log includes the session id and exact path.
//!
//! Init may omit context/session_id: `{"op":"init"}` uses this endpoint's
//! current session and returns it in `result.session_id`. Other requests carry
//! `"context":{"session_id":"..."}`. For example:
//! `{"op":"init","context":{"session_id":"..."}}` binds the backend;
//! `{"op":"init","config":{"save_mode":"current"}}` overrides the startup mode.
//! Each init resolves the mode afresh: init override > startup config > next.
//! Strategy construction is deferred until init; omitted options on re-init
//! fall back to startup configuration.
//! `{"op":"save","context":{"session_id":"..."}}` enqueues a save.
//! Restore also carries `snapshot_id` and `mode`; delete_snapshot carries
//! `snapshot_id`. Release needs only the shared context.
//! Responses: `{"ok":true,"result":...}` or `{"ok":false,"error":"..."}`.
//! Save returns `{id, session_id}`; restore/delete return `{accepted:true}`.
//! Commands execute at the next loop boundary, so an idle TUI needs another
//! turn before queued commands execute. A returned snapshot id is not yet ready.
//! `{"op":"list_checkpoint","context":{"session_id":"..."},"access_mode":"last_n"}`
//! lists Pending/Ready IDs newest first; optional `count` defaults to 10,
//! zero returns no IDs, and any negative count returns all accessible IDs.
//! Restore mode is required: `ensure_safe_mode` accepts Ready; `promise_mode`
//! accepts Pending and Ready. Both reject Failed, deleted and unknown IDs.
//! Release closes the API gate immediately and queues cooling after accepted
//! commands. Re-init queues activation on the same driver. Release's response
//! acknowledges the barrier, not completion of cleanup; MemoryStorage data stays.
//! No explicit handle id is required for this sequential socket protocol.
use super::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{Error, ErrorKind},
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

/// Owns a local session's socket and reusable loop driver. Dropping it closes
/// the listener and destroys its checkpoint backend/storage. Socket `release`
/// only cools the handle and does not perform this session teardown.
pub struct UnixCheckpointEndpoint {
    pub(super) backend: Arc<super::backend::QueuedCheckpointBackend>,
    pub driver: Arc<CheckpointDriver>,
    path: PathBuf,
    task: JoinHandle<()>,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Init {
        #[serde(default)]
        context: SnapshotContext,
        #[serde(default = "empty_init_config")]
        config: Value,
    },
    Save {
        context: SnapshotContext,
    },
    ListCheckpoint {
        context: SnapshotContext,
        access_mode: AccessMode,
        count: Option<i64>,
    },
    Restore {
        context: SnapshotContext,
        snapshot_id: SnapshotId,
        mode: RestoreMode,
    },
    DeleteSnapshot {
        context: SnapshotContext,
        snapshot_id: SnapshotId,
    },
    Release {
        context: SnapshotContext,
    },
}
fn empty_init_config() -> Value {
    json!({})
}

impl UnixCheckpointEndpoint {
    pub async fn bind(
        session_id: String,
        config: CheckpointConfig,
    ) -> Result<Self, CheckpointError> {
        let path = config
            .unix_socket
            .as_ref()
            .map(|socket| socket.endpoint.clone())
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "checkpoint.unix_socket.endpoint is required",
                )
            })?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        remove_stale_socket(&path).await?;
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let backend = super::factory::create_backend(config)?;
        let driver = Arc::new(CheckpointDriver::default());
        let task_backend = backend.clone();
        let task_driver = driver.clone();
        let task = tokio::spawn(async move {
            // Handle requests in accepted order. Core queue ordering also covers
            // callers using the Rust interface directly.
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                // A client that never finishes a line must not block all control requests.
                match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    handle(stream, &session_id, &task_backend, &task_driver),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => tracing::warn!(%error, "checkpoint socket request failed"),
                    Err(_) => tracing::warn!("checkpoint socket request timed out"),
                }
            }
        });
        Ok(Self {
            backend,
            driver,
            path,
            task,
        })
    }
    pub(super) async fn close(mut self) {
        self.backend.close();
        self.task.abort();
        let _ = (&mut self.task).await;
        self.driver.close().await;
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}
impl Drop for UnixCheckpointEndpoint {
    fn drop(&mut self) {
        self.task.abort();
        self.backend.close();
        let _ = std::fs::remove_file(&self.path);
    }
}
/// Only a refused connection to an unchanged socket path proves it stale.
async fn remove_stale_socket(path: &Path) -> Result<(), std::io::Error> {
    let original = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !original.file_type().is_socket() {
        return Err(Error::new(
            ErrorKind::AddrInUse,
            "checkpoint endpoint exists and is not a socket",
        ));
    }
    match tokio::time::timeout(std::time::Duration::from_secs(1), UnixStream::connect(path)).await {
        Ok(Ok(_)) => {
            return Err(Error::new(
                ErrorKind::AddrInUse,
                "checkpoint endpoint is already listening",
            ))
        }
        Ok(Err(error)) if error.kind() == ErrorKind::ConnectionRefused => {}
        Ok(Err(error)) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(Error::new(
                ErrorKind::TimedOut,
                "checkpoint endpoint probe timed out",
            ))
        }
    }
    let current = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if (original.dev(), original.ino()) != (current.dev(), current.ino()) {
        return Err(Error::new(
            ErrorKind::AddrInUse,
            "checkpoint endpoint changed during probe",
        ));
    }
    std::fs::remove_file(path)
}

async fn handle(
    mut stream: UnixStream,
    session: &str,
    backend: &Arc<super::backend::QueuedCheckpointBackend>,
    driver: &Arc<CheckpointDriver>,
) -> Result<(), std::io::Error> {
    let mut line = String::new();
    if BufReader::new((&mut stream).take(65536))
        .read_line(&mut line)
        .await?
        == 0
    {
        // A liveness probe connects and closes without sending a request.
        return Ok(());
    }
    let response = match serde_json::from_str::<Request>(&line) {
        Ok(request) => match dispatch(request, session, backend, driver).await {
            Ok(value) => json!({"ok": true, "result": value}),
            Err(error) => json!({"ok": false, "error": error.to_string()}),
        },
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    };
    let mut bytes = serde_json::to_vec(&response)?;
    bytes.push(b'\n');
    stream.write_all(&bytes).await
}
async fn dispatch(
    request: Request,
    session: &str,
    backend: &Arc<super::backend::QueuedCheckpointBackend>,
    driver: &Arc<CheckpointDriver>,
) -> Result<Value, CheckpointError> {
    match request {
        Request::Init {
            mut context,
            config,
        } => {
            if context.session_id.is_empty() {
                context.session_id = session.to_owned();
            }
            if context.session_id != session {
                return Err(CheckpointError::SessionMismatch);
            }
            let binding = backend.init(context, config).await?;
            if let Some(initial_driver) = binding.driver {
                driver.attach(initial_driver).await;
            }
            Ok(json!(binding.info))
        }
        Request::Save { context } => Ok(json!(backend.save(context)?)),
        Request::ListCheckpoint {
            context,
            access_mode,
            count,
        } => Ok(json!(backend.list_checkpoint(
            context,
            access_mode,
            count
        )?)),
        Request::Restore {
            context,
            snapshot_id,
            mode,
        } => {
            backend
                .restore(context, RestoreRequest { snapshot_id, mode })
                .await?;
            Ok(json!({"accepted": true}))
        }
        Request::DeleteSnapshot {
            context,
            snapshot_id,
        } => {
            backend.delete_snapshot(context, &snapshot_id).await?;
            Ok(json!({"accepted": true}))
        }
        Request::Release { context } => {
            backend.release(context);
            Ok(Value::Null)
        }
    }
}
