use agent_contracts::backend::{
    BackendId, BackendLifecycleReason, BackendResourceLimits, OperationBackendBuildError,
    OperationError,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{
    backend_tree_node, build_backend, current_time_ms, delete_backend_instance, detach_from_parent,
    e2b, expires_at_ms_from_timeout, forked_provider_options, hash_config, metadata_matches_filter,
    new_backend_id, requested_backend_id, resolve_backend_config, resolve_session_backend_config,
    workspace_root_string, BackendCheckoutRequest, BackendCheckoutResult, BackendCheckpointRef,
    BackendCheckpointRequest, BackendCheckpointResult, BackendCheckpointSnapshotDeleteRequest,
    BackendCheckpointSnapshotDeleteResult, BackendConnectRequest, BackendCreateRequest,
    BackendEnsureSessionRequest, BackendError, BackendForkRequest, BackendForkResult, BackendInfo,
    BackendInstanceEntry, BackendLease, BackendLineageEntry, BackendListFilter, BackendTreeNode,
    BuildBackendInput,
};

#[derive(Default)]
pub struct BackendManager {
    pub(super) state: Mutex<BackendManagerState>,
}

#[derive(Default)]
pub(super) struct BackendManagerState {
    pub(super) backends: HashMap<BackendId, BackendInstanceEntry>,
    pub(super) session_index: HashMap<String, BackendId>,
}

fn checkout_metadata(
    metadata: Value,
    checkpoint: &BackendCheckpointRef,
    child_backend_id: &str,
) -> Value {
    let mut object = match metadata {
        Value::Object(object) => object,
        Value::Null => serde_json::Map::new(),
        other => {
            let mut object = serde_json::Map::new();
            object.insert("user_metadata".to_string(), other);
            object
        }
    };
    object.insert(
        "xiaoo_checkpoint_id".to_string(),
        Value::String(checkpoint.checkpoint_id.clone()),
    );
    if let Some(snapshot_id) = checkpoint.provider_snapshot_id.as_ref() {
        object.insert(
            "xiaoo_provider_snapshot_id".to_string(),
            Value::String(snapshot_id.clone()),
        );
    }
    if let Some(source_backend_id) = checkpoint.source_backend_id.as_ref() {
        object.insert(
            "xiaoo_checkpoint_source_backend_id".to_string(),
            Value::String(source_backend_id.clone()),
        );
    }
    object.insert(
        "xiaoo_checkout_child_backend_id".to_string(),
        Value::String(child_backend_id.to_string()),
    );
    Value::Object(object)
}

fn insert_checkout_child(
    state: &mut BackendManagerState,
    source_backend_id: Option<&str>,
    child_backend_id: &BackendId,
    child_session_id: Option<String>,
    child_entry: BackendInstanceEntry,
) -> Result<(), BackendError> {
    if let Some(source_backend_id) = source_backend_id {
        if let Some(parent) = state
            .backends
            .get_mut(&BackendId(source_backend_id.to_string()))
        {
            parent
                .lineage
                .children_backend_ids
                .insert(child_backend_id.0.clone(), ());
        }
    }
    if let Some(session_id) = child_session_id {
        state
            .session_index
            .insert(session_id, child_backend_id.clone());
    }
    state.backends.insert(child_backend_id.clone(), child_entry);
    Ok(())
}

fn resolve_checkpoint_backend_id(
    state: &BackendManagerState,
    backend_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<BackendId, BackendError> {
    let by_backend = backend_id
        .filter(|value| !value.trim().is_empty())
        .map(|value| BackendId(value.to_string()));
    let by_session = session_id
        .filter(|value| !value.trim().is_empty())
        .map(|session_id| {
            state
                .session_index
                .get(session_id)
                .cloned()
                .ok_or_else(|| BackendError::NotFound {
                    backend_id: format!("session:{session_id}"),
                })
        })
        .transpose()?;

    match (by_backend, by_session) {
        (Some(backend_id), Some(session_backend_id)) if backend_id != session_backend_id => {
            Err(BackendError::Conflict {
                message: format!(
                    "backend_id {backend_id} does not match session_id backend {session_backend_id}"
                ),
            })
        }
        (Some(backend_id), _) | (_, Some(backend_id)) => Ok(backend_id),
        (None, None) => Err(BackendError::InvalidRequest {
            message: "checkpoint requires backend_id or session_id".to_string(),
        }),
    }
}

impl BackendManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_backend(
        &self,
        request: BackendCreateRequest,
    ) -> Result<BackendInfo, BackendError> {
        let config = resolve_backend_config(request.provider.clone(), request.options)?;
        let backend_id = requested_backend_id(request.backend_id)?;
        let workspace_root = workspace_root_string(&request.workspace_root)
            .map_err(BackendError::from_build_error)?;
        let config_hash = hash_config(&config);
        let expires_at_ms = request.timeout.map(expires_at_ms_from_timeout);

        let mut state = self.state.lock().await;
        if state.backends.contains_key(&backend_id) {
            return Err(BackendError::Conflict {
                message: format!("backend_id {backend_id} already exists"),
            });
        }
        if let Some(session_id) = request.session_id.as_ref() {
            if let Some(existing_backend_id) = state.session_index.get(session_id) {
                return Err(BackendError::Conflict {
                    message: format!(
                        "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                    ),
                });
            }
        }

        let entry = build_backend(BuildBackendInput {
            backend_id: backend_id.clone(),
            config,
            workspace_root_text: workspace_root,
            config_hash,
            session_id_for_instance: request
                .session_id
                .clone()
                .unwrap_or_else(|| backend_id.0.clone()),
            session_id: request.session_id,
            resource_limits: request.resource_limits,
            metadata: request.metadata,
            expires_at_ms,
            lineage: BackendLineageEntry::default(),
            backend_checkpoint: None,
        })
        .await?;
        let info = entry.info();
        let session_ids = entry.session_ids.keys().cloned().collect::<Vec<_>>();
        for session_id in session_ids {
            state.session_index.insert(session_id, backend_id.clone());
        }
        state.backends.insert(backend_id, entry);
        Ok(info)
    }

    pub async fn connect_backend(
        &self,
        backend_id: &str,
        request: BackendConnectRequest,
    ) -> Result<BackendInfo, BackendError> {
        let mut state = self.state.lock().await;
        let backend_id = BackendId(backend_id.to_string());
        if let Some(session_id) = request.session_id.as_ref() {
            if let Some(existing_backend_id) = state.session_index.get(session_id) {
                if existing_backend_id != &backend_id {
                    return Err(BackendError::Conflict {
                        message: format!(
                            "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                        ),
                    });
                }
            }
        }
        let session_id = request.session_id;
        let info = {
            let entry =
                state
                    .backends
                    .get_mut(&backend_id)
                    .ok_or_else(|| BackendError::NotFound {
                        backend_id: backend_id.0.clone(),
                    })?;
            if let Some(timeout) = request.timeout {
                entry.expires_at_ms = Some(expires_at_ms_from_timeout(timeout));
            }
            if let Some(session_id) = session_id.as_ref() {
                entry.session_ids.insert(session_id.clone(), ());
            }
            entry.info()
        };
        if let Some(session_id) = session_id {
            state.session_index.insert(session_id, backend_id);
        }
        Ok(info)
    }

    pub async fn get_backend(&self, backend_id: &str) -> Result<BackendInfo, BackendError> {
        let state = self.state.lock().await;
        state
            .backends
            .get(&BackendId(backend_id.to_string()))
            .map(BackendInstanceEntry::info)
            .ok_or_else(|| BackendError::NotFound {
                backend_id: backend_id.to_string(),
            })
    }

    pub async fn list_backends(&self, filter: BackendListFilter) -> Vec<BackendInfo> {
        let state = self.state.lock().await;
        state
            .backends
            .values()
            .filter(|entry| metadata_matches_filter(&entry.instance.metadata, &filter.metadata))
            .map(BackendInstanceEntry::info)
            .collect()
    }

    pub async fn list_backend_trees(&self) -> Vec<BackendTreeNode> {
        let state = self.state.lock().await;
        let mut roots = state
            .backends
            .iter()
            .filter_map(|(backend_id, entry)| {
                let parent_is_live = entry
                    .lineage
                    .parent_backend_id
                    .as_ref()
                    .is_some_and(|parent| state.backends.contains_key(parent));
                (!parent_is_live).then(|| backend_id.clone())
            })
            .collect::<Vec<_>>();
        roots.sort_by(|a, b| a.0.cmp(&b.0));
        roots
            .into_iter()
            .filter_map(|backend_id| backend_tree_node(&state, &backend_id))
            .collect()
    }

    pub async fn delete_backend(&self, backend_id: &str) -> Result<(), BackendError> {
        let removed = {
            let mut state = self.state.lock().await;
            let backend_id = BackendId(backend_id.to_string());
            let entry =
                state
                    .backends
                    .remove(&backend_id)
                    .ok_or_else(|| BackendError::NotFound {
                        backend_id: backend_id.0.clone(),
                    })?;
            for session_id in entry.session_ids.keys() {
                state.session_index.remove(session_id);
            }
            detach_from_parent(&mut state, &backend_id, &entry);
            entry
        };
        delete_backend_instance(removed, BackendLifecycleReason::UserRequested)
            .await
            .map_err(BackendError::from_operation_error)
    }

    pub async fn checkpoint_backend(
        &self,
        request: BackendCheckpointRequest,
    ) -> Result<BackendCheckpointResult, BackendError> {
        let source = {
            let state = self.state.lock().await;
            let backend_id = resolve_checkpoint_backend_id(
                &state,
                request.backend_id.as_deref(),
                request.session_id.as_deref(),
            )?;
            let entry = state
                .backends
                .get(&backend_id)
                .ok_or_else(|| BackendError::NotFound {
                    backend_id: backend_id.0.clone(),
                })?;
            if !entry.dirty_tracker.is_dirty() {
                if let Some(checkpoint) = entry.dirty_tracker.checkpoint() {
                    return Ok(BackendCheckpointResult {
                        backend: entry.info(),
                        checkpoint,
                        reused: true,
                    });
                }
            }
            (
                backend_id,
                entry.config.clone(),
                entry.workspace_root.clone(),
                entry.instance.instance_id.0.clone(),
            )
        };

        let (backend_id, config, workspace_root, instance_id) = source;
        let now_ms = current_time_ms();
        let (provider_snapshot_id, provider_snapshot_names) = if config.kind == "e2b" {
            let snapshot = e2b::create_snapshot(e2b::E2bSnapshotInput {
                provider_options: config.options.clone(),
                sandbox_id: instance_id,
                name: request.name.clone(),
            })
            .await?;
            (Some(snapshot.snapshot_id), snapshot.names)
        } else {
            (None, Vec::new())
        };

        let checkpoint = BackendCheckpointRef {
            checkpoint_id: format!("bcp_{}", uuid::Uuid::new_v4().simple()),
            provider: config.kind,
            source_backend_id: Some(backend_id.0.clone()),
            provider_snapshot_id,
            provider_snapshot_names,
            workspace_root,
            name: request.name,
            metadata: request.metadata,
            created_at_ms: now_ms,
            provider_options: config.options,
        };

        let backend = {
            let state = self.state.lock().await;
            let entry = state
                .backends
                .get(&backend_id)
                .ok_or_else(|| BackendError::NotFound {
                    backend_id: backend_id.0.clone(),
                })?;
            entry.dirty_tracker.set_checkpoint(checkpoint.clone());
            entry.info()
        };

        Ok(BackendCheckpointResult {
            backend,
            checkpoint,
            reused: false,
        })
    }

    pub async fn delete_checkpoint_snapshot(
        &self,
        request: BackendCheckpointSnapshotDeleteRequest,
    ) -> Result<BackendCheckpointSnapshotDeleteResult, BackendError> {
        let checkpoint = request.checkpoint;
        let provider = checkpoint.provider.clone();
        let provider_snapshot_id = checkpoint.provider_snapshot_id.clone();
        let provider_snapshot_names = checkpoint.provider_snapshot_names.clone();

        let Some(snapshot_id) = provider_snapshot_id.as_deref() else {
            return Ok(BackendCheckpointSnapshotDeleteResult {
                checkpoint_id: checkpoint.checkpoint_id,
                provider,
                provider_snapshot_id,
                provider_snapshot_names,
                deleted: false,
            });
        };

        if provider != "e2b" {
            return Err(BackendError::UnsupportedBackend {
                kind: format!("{provider}:delete_snapshot"),
            });
        }

        let deleted = e2b::delete_snapshot(e2b::E2bDeleteSnapshotInput {
            provider_options: checkpoint.provider_options.clone(),
            snapshot_id: snapshot_id.to_string(),
        })
        .await?;

        if let Some(source_backend_id) = checkpoint.source_backend_id.as_ref() {
            let state = self.state.lock().await;
            if let Some(entry) = state.backends.get(&BackendId(source_backend_id.clone())) {
                entry
                    .dirty_tracker
                    .clear_checkpoint_if_matches(&checkpoint.checkpoint_id);
            }
        }

        Ok(BackendCheckpointSnapshotDeleteResult {
            checkpoint_id: checkpoint.checkpoint_id,
            provider,
            provider_snapshot_id,
            provider_snapshot_names,
            deleted,
        })
    }

    pub async fn checkout_backend(
        &self,
        request: BackendCheckoutRequest,
    ) -> Result<BackendCheckoutResult, BackendError> {
        let Some(snapshot_id) = request.checkpoint.provider_snapshot_id.as_deref() else {
            return Err(BackendError::UnsupportedBackend {
                kind: format!("{}:checkout", request.checkpoint.provider),
            });
        };
        if request.checkpoint.provider != "e2b" {
            return Err(BackendError::UnsupportedBackend {
                kind: format!("{}:checkout", request.checkpoint.provider),
            });
        }

        let child_backend_id = requested_backend_id(request.backend_id.clone())?;
        let child_session_id = request.session_id.clone();
        let expires_at_ms = request.timeout.map(expires_at_ms_from_timeout);
        {
            let state = self.state.lock().await;
            if state.backends.contains_key(&child_backend_id) {
                return Err(BackendError::Conflict {
                    message: format!("backend_id {child_backend_id} already exists"),
                });
            }
            if let Some(session_id) = child_session_id.as_ref() {
                if let Some(existing_backend_id) = state.session_index.get(session_id) {
                    return Err(BackendError::Conflict {
                        message: format!(
                            "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                        ),
                    });
                }
            }
        }

        let mut create_config =
            super::GatewayBackendConfig::new("e2b", request.checkpoint.provider_options.clone());
        create_config.options = forked_provider_options(
            &request.checkpoint.provider_options,
            request.options.as_ref(),
            snapshot_id,
        );
        let create_config_hash = hash_config(&create_config);
        let stored_config =
            super::GatewayBackendConfig::new("e2b", request.checkpoint.provider_options.clone());
        let stored_config_hash = hash_config(&stored_config);
        let metadata = checkout_metadata(
            request.metadata,
            &request.checkpoint,
            child_backend_id.0.as_str(),
        );
        let lineage = BackendLineageEntry {
            parent_backend_id: request
                .checkpoint
                .source_backend_id
                .as_ref()
                .map(|id| BackendId(id.clone())),
            children_backend_ids: BTreeMap::new(),
            forked_from_snapshot_id: Some(snapshot_id.to_string()),
            forked_snapshot_names: request.checkpoint.provider_snapshot_names.clone(),
            forked_at_ms: Some(current_time_ms()),
        };

        let mut child_entry = build_backend(BuildBackendInput {
            backend_id: child_backend_id.clone(),
            config: create_config,
            workspace_root_text: request.checkpoint.workspace_root.clone(),
            config_hash: create_config_hash,
            session_id_for_instance: child_session_id
                .clone()
                .unwrap_or_else(|| child_backend_id.0.clone()),
            session_id: child_session_id.clone(),
            resource_limits: request.resource_limits,
            metadata,
            expires_at_ms,
            lineage,
            backend_checkpoint: Some(request.checkpoint.clone()),
        })
        .await?;
        child_entry.config = stored_config;
        child_entry.config_hash = stored_config_hash;

        let child_info = child_entry.info();
        let mut child_entry = Some(child_entry);
        let insert_result = {
            let mut state = self.state.lock().await;
            if state.backends.contains_key(&child_backend_id) {
                Err(BackendError::Conflict {
                    message: format!("backend_id {child_backend_id} already exists"),
                })
            } else if let Some(session_id) = child_session_id.as_ref() {
                if let Some(existing_backend_id) = state.session_index.get(session_id) {
                    Err(BackendError::Conflict {
                        message: format!(
                            "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                        ),
                    })
                } else {
                    insert_checkout_child(
                        &mut state,
                        request.checkpoint.source_backend_id.as_deref(),
                        &child_backend_id,
                        child_session_id.clone(),
                        child_entry.take().expect("child entry should be present"),
                    )
                }
            } else {
                insert_checkout_child(
                    &mut state,
                    request.checkpoint.source_backend_id.as_deref(),
                    &child_backend_id,
                    None,
                    child_entry.take().expect("child entry should be present"),
                )
            }
        };

        if let Err(error) = insert_result {
            if let Some(entry) = child_entry {
                delete_backend_instance(entry, BackendLifecycleReason::UserRequested)
                    .await
                    .map_err(BackendError::from_operation_error)?;
            }
            return Err(error);
        }

        Ok(BackendCheckoutResult {
            backend: child_info,
            checkpoint: request.checkpoint,
        })
    }

    pub async fn fork_backend(
        &self,
        request: BackendForkRequest,
    ) -> Result<BackendForkResult, BackendError> {
        let checkpoint = self
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: request.parent_backend_id,
                session_id: request.parent_session_id,
                name: request.snapshot_name,
                metadata: Value::Null,
            })
            .await?;
        let checkout = self
            .checkout_backend(BackendCheckoutRequest {
                checkpoint: checkpoint.checkpoint.clone(),
                backend_id: request.backend_id,
                session_id: request.session_id,
                timeout: request.timeout,
                metadata: request.metadata,
                resource_limits: request.resource_limits,
                options: request.options,
            })
            .await?;
        let snapshot_id = checkpoint
            .checkpoint
            .provider_snapshot_id
            .clone()
            .unwrap_or_else(|| checkpoint.checkpoint.checkpoint_id.clone());

        Ok(BackendForkResult {
            parent: checkpoint.backend,
            child: checkout.backend,
            snapshot_id,
            snapshot_names: checkpoint.checkpoint.provider_snapshot_names,
        })
    }

    pub async fn ensure_session_backend(
        &self,
        request: BackendEnsureSessionRequest,
    ) -> Result<BackendLease, OperationBackendBuildError> {
        let config = resolve_session_backend_config(request.config.clone())?;
        let workspace_root = workspace_root_string(&request.workspace_root)?;
        let config_hash = hash_config(&config);
        let mut state = self.state.lock().await;

        if let Some(existing_backend_id) = state.session_index.get(&request.session_id) {
            let entry = state.backends.get(existing_backend_id).ok_or_else(|| {
                OperationBackendBuildError::BuildFailed {
                    message: format!(
                        "session {} is bound to missing backend {}",
                        request.session_id, existing_backend_id
                    ),
                }
            })?;
            if entry.workspace_root != workspace_root || entry.config_hash != config_hash {
                return Err(OperationBackendBuildError::BuildFailed {
                    message: format!(
                        "session {} is already bound to backend {} with different workspace or config",
                        request.session_id, existing_backend_id
                    ),
                });
            }
            return Ok(BackendLease::new(
                Arc::clone(&entry.backend),
                entry.instance.clone(),
            ));
        }

        let backend_id = new_backend_id();
        let entry = build_backend(BuildBackendInput {
            backend_id: backend_id.clone(),
            config,
            workspace_root_text: workspace_root,
            config_hash,
            session_id_for_instance: request.session_id.clone(),
            session_id: Some(request.session_id.clone()),
            resource_limits: BackendResourceLimits::default(),
            metadata: Value::Null,
            expires_at_ms: None,
            lineage: BackendLineageEntry::default(),
            backend_checkpoint: None,
        })
        .await
        .map_err(BackendError::into_build_error)?;
        let backend = Arc::clone(&entry.backend);
        let instance = entry.instance.clone();

        state
            .session_index
            .insert(request.session_id, backend_id.clone());
        state.backends.insert(backend_id, entry);
        Ok(BackendLease::new(backend, instance))
    }

    pub async fn lease_bound_session(
        &self,
        session_id: &str,
    ) -> Result<BackendLease, BackendError> {
        let state = self.state.lock().await;
        let backend_id =
            state
                .session_index
                .get(session_id)
                .ok_or_else(|| BackendError::NotFound {
                    backend_id: format!("session:{session_id}"),
                })?;
        let entry = state
            .backends
            .get(backend_id)
            .ok_or_else(|| BackendError::NotFound {
                backend_id: backend_id.0.clone(),
            })?;
        Ok(BackendLease::new(
            Arc::clone(&entry.backend),
            entry.instance.clone(),
        ))
    }

    pub async fn release_session(&self, session_id: &str) -> Result<(), OperationError> {
        let removed = {
            let mut state = self.state.lock().await;
            let Some(backend_id) = state.session_index.remove(session_id) else {
                return Ok(());
            };
            let Some(entry) = state.backends.get_mut(&backend_id) else {
                return Ok(());
            };
            entry.session_ids.remove(session_id);
            if entry.session_ids.is_empty() {
                let removed = state.backends.remove(&backend_id);
                if let Some(entry) = removed.as_ref() {
                    detach_from_parent(&mut state, &backend_id, entry);
                }
                removed
            } else {
                None
            }
        };

        if let Some(instance) = removed {
            delete_backend_instance(instance, BackendLifecycleReason::SessionClose).await?;
        }
        Ok(())
    }

    pub async fn shutdown_all(&self) -> Result<(), OperationError> {
        let removed = {
            let mut state = self.state.lock().await;
            state.session_index.clear();
            state
                .backends
                .drain()
                .map(|(_, instance)| instance)
                .collect::<Vec<_>>()
        };

        for instance in removed {
            delete_backend_instance(instance, BackendLifecycleReason::DaemonShutdown).await?;
        }
        Ok(())
    }
}
