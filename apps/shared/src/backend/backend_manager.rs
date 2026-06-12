use agent_contracts::backend::{
    BackendId, BackendLifecycleReason, BackendResourceLimits, OperationBackendBuildError,
    OperationError,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{
    build_managed_backend, current_time_ms, delete_backend_instance, detach_from_parent, e2b,
    expires_at_ms_from_timeout, fork_metadata, forked_provider_options, hash_config,
    insert_forked_child, metadata_matches_filter, new_backend_id, requested_backend_id,
    resolve_parent_backend_id, resolve_sandbox_backend_config, resolve_session_backend_config,
    sandbox_tree_node, workspace_root_string, BackendEnsureSessionRequest, BackendInstanceEntry,
    BackendLease, BuildSandboxInput, ParentForkSource, SandboxConnectRequest, SandboxCreateRequest,
    SandboxError, SandboxForkRequest, SandboxForkResult, SandboxInfo, SandboxLineageEntry,
    SandboxListFilter, SandboxTreeNode,
};

#[derive(Default)]
pub struct BackendManager {
    pub(super) state: Mutex<BackendManagerState>,
}

#[derive(Default)]
pub(super) struct BackendManagerState {
    pub(super) sandboxes: HashMap<BackendId, BackendInstanceEntry>,
    pub(super) session_index: HashMap<String, BackendId>,
}

impl BackendManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_sandbox(
        &self,
        request: SandboxCreateRequest,
    ) -> Result<SandboxInfo, SandboxError> {
        let config = resolve_sandbox_backend_config(request.provider.clone(), request.options)?;
        let backend_id = requested_backend_id(request.backend_id)?;
        let workspace_root = workspace_root_string(&request.workspace_root)
            .map_err(SandboxError::from_build_error)?;
        let config_hash = hash_config(&config);
        let expires_at_ms = request.timeout.map(expires_at_ms_from_timeout);

        let mut state = self.state.lock().await;
        if state.sandboxes.contains_key(&backend_id) {
            return Err(SandboxError::Conflict {
                message: format!("backend_id {backend_id} already exists"),
            });
        }
        if let Some(session_id) = request.session_id.as_ref() {
            if let Some(existing_backend_id) = state.session_index.get(session_id) {
                return Err(SandboxError::Conflict {
                    message: format!(
                        "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                    ),
                });
            }
        }

        let entry = build_managed_backend(BuildSandboxInput {
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
            lineage: SandboxLineageEntry::default(),
        })
        .await?;
        let info = entry.info();
        let session_ids = entry.session_ids.keys().cloned().collect::<Vec<_>>();
        for session_id in session_ids {
            state.session_index.insert(session_id, backend_id.clone());
        }
        state.sandboxes.insert(backend_id, entry);
        Ok(info)
    }

    pub async fn connect_sandbox(
        &self,
        backend_id: &str,
        request: SandboxConnectRequest,
    ) -> Result<SandboxInfo, SandboxError> {
        let mut state = self.state.lock().await;
        let backend_id = BackendId(backend_id.to_string());
        if let Some(session_id) = request.session_id.as_ref() {
            if let Some(existing_backend_id) = state.session_index.get(session_id) {
                if existing_backend_id != &backend_id {
                    return Err(SandboxError::Conflict {
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
                    .sandboxes
                    .get_mut(&backend_id)
                    .ok_or_else(|| SandboxError::NotFound {
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

    pub async fn get_sandbox(&self, backend_id: &str) -> Result<SandboxInfo, SandboxError> {
        let state = self.state.lock().await;
        state
            .sandboxes
            .get(&BackendId(backend_id.to_string()))
            .map(BackendInstanceEntry::info)
            .ok_or_else(|| SandboxError::NotFound {
                backend_id: backend_id.to_string(),
            })
    }

    pub async fn list_sandboxes(&self, filter: SandboxListFilter) -> Vec<SandboxInfo> {
        let state = self.state.lock().await;
        state
            .sandboxes
            .values()
            .filter(|entry| metadata_matches_filter(&entry.instance.metadata, &filter.metadata))
            .map(BackendInstanceEntry::info)
            .collect()
    }

    pub async fn list_sandbox_trees(&self) -> Vec<SandboxTreeNode> {
        let state = self.state.lock().await;
        let mut roots = state
            .sandboxes
            .iter()
            .filter_map(|(backend_id, entry)| {
                let parent_is_live = entry
                    .lineage
                    .parent_backend_id
                    .as_ref()
                    .is_some_and(|parent| state.sandboxes.contains_key(parent));
                (!parent_is_live).then(|| backend_id.clone())
            })
            .collect::<Vec<_>>();
        roots.sort_by(|a, b| a.0.cmp(&b.0));
        roots
            .into_iter()
            .filter_map(|backend_id| sandbox_tree_node(&state, &backend_id))
            .collect()
    }

    pub async fn delete_sandbox(&self, backend_id: &str) -> Result<(), SandboxError> {
        let removed = {
            let mut state = self.state.lock().await;
            let backend_id = BackendId(backend_id.to_string());
            let entry =
                state
                    .sandboxes
                    .remove(&backend_id)
                    .ok_or_else(|| SandboxError::NotFound {
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
            .map_err(SandboxError::from_operation_error)
    }

    pub async fn fork_sandbox(
        &self,
        request: SandboxForkRequest,
    ) -> Result<SandboxForkResult, SandboxError> {
        let child_backend_id = requested_backend_id(request.backend_id.clone())?;
        let child_session_id = request.session_id.clone();
        let expires_at_ms = request.timeout.map(expires_at_ms_from_timeout);

        let parent = {
            let state = self.state.lock().await;
            let parent_backend_id = resolve_parent_backend_id(&state, &request)?;
            if state.sandboxes.contains_key(&child_backend_id) {
                return Err(SandboxError::Conflict {
                    message: format!("backend_id {child_backend_id} already exists"),
                });
            }
            if let Some(session_id) = child_session_id.as_ref() {
                if let Some(existing_backend_id) = state.session_index.get(session_id) {
                    return Err(SandboxError::Conflict {
                        message: format!(
                            "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                        ),
                    });
                }
            }
            let entry =
                state
                    .sandboxes
                    .get(&parent_backend_id)
                    .ok_or_else(|| SandboxError::NotFound {
                        backend_id: parent_backend_id.0.clone(),
                    })?;
            if entry.config.kind != "e2b" {
                return Err(SandboxError::UnsupportedBackend {
                    kind: format!("{}:fork", entry.config.kind),
                });
            }
            ParentForkSource {
                backend_id: parent_backend_id,
                config: entry.config.clone(),
                workspace_root: entry.workspace_root.clone(),
                instance_id: entry.instance.instance_id.0.clone(),
            }
        };

        let snapshot = e2b::create_snapshot(e2b::E2bSnapshotInput {
            provider_options: parent.config.options.clone(),
            sandbox_id: parent.instance_id.clone(),
            name: request.snapshot_name.clone(),
        })
        .await?;

        let mut child_config = parent.config.clone();
        child_config.options = forked_provider_options(
            &parent.config.options,
            request.options.as_ref(),
            snapshot.snapshot_id.as_str(),
        );
        let child_create_config_hash = hash_config(&child_config);
        let child_stored_config_hash = hash_config(&parent.config);
        let child_metadata = fork_metadata(
            request.metadata,
            &parent,
            child_backend_id.0.as_str(),
            snapshot.snapshot_id.as_str(),
        );
        let lineage = SandboxLineageEntry {
            parent_backend_id: Some(parent.backend_id.clone()),
            children_backend_ids: BTreeMap::new(),
            forked_from_snapshot_id: Some(snapshot.snapshot_id.clone()),
            forked_snapshot_names: snapshot.names.clone(),
            forked_at_ms: Some(current_time_ms()),
        };

        let mut child_entry = build_managed_backend(BuildSandboxInput {
            backend_id: child_backend_id.clone(),
            config: child_config,
            workspace_root_text: parent.workspace_root.clone(),
            config_hash: child_create_config_hash,
            session_id_for_instance: child_session_id
                .clone()
                .unwrap_or_else(|| child_backend_id.0.clone()),
            session_id: child_session_id.clone(),
            resource_limits: request.resource_limits,
            metadata: child_metadata,
            expires_at_ms,
            lineage,
        })
        .await?;
        child_entry.config = parent.config.clone();
        child_entry.config_hash = child_stored_config_hash;

        let child_info = child_entry.info();
        let mut child_entry = Some(child_entry);
        let insert_result = {
            let mut state = self.state.lock().await;
            if state.sandboxes.contains_key(&child_backend_id) {
                Err(SandboxError::Conflict {
                    message: format!("backend_id {child_backend_id} already exists"),
                })
            } else if let Some(session_id) = child_session_id.as_ref() {
                if let Some(existing_backend_id) = state.session_index.get(session_id) {
                    Err(SandboxError::Conflict {
                        message: format!(
                            "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                        ),
                    })
                } else {
                    insert_forked_child(
                        &mut state,
                        &parent.backend_id,
                        &child_backend_id,
                        child_session_id.clone(),
                        child_entry.take().expect("child entry should be present"),
                    )
                }
            } else {
                insert_forked_child(
                    &mut state,
                    &parent.backend_id,
                    &child_backend_id,
                    None,
                    child_entry.take().expect("child entry should be present"),
                )
            }
        };

        let parent_info = match insert_result {
            Ok(parent_info) => parent_info,
            Err(error) => {
                if let Some(entry) = child_entry {
                    delete_backend_instance(entry, BackendLifecycleReason::UserRequested)
                        .await
                        .map_err(SandboxError::from_operation_error)?;
                }
                return Err(error);
            }
        };

        Ok(SandboxForkResult {
            parent: parent_info,
            child: child_info,
            snapshot_id: snapshot.snapshot_id,
            snapshot_names: snapshot.names,
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
            let entry = state.sandboxes.get(existing_backend_id).ok_or_else(|| {
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
        let entry = build_managed_backend(BuildSandboxInput {
            backend_id: backend_id.clone(),
            config,
            workspace_root_text: workspace_root,
            config_hash,
            session_id_for_instance: request.session_id.clone(),
            session_id: Some(request.session_id.clone()),
            resource_limits: BackendResourceLimits::default(),
            metadata: Value::Null,
            expires_at_ms: None,
            lineage: SandboxLineageEntry::default(),
        })
        .await
        .map_err(SandboxError::into_build_error)?;
        let backend = Arc::clone(&entry.backend);
        let instance = entry.instance.clone();

        state
            .session_index
            .insert(request.session_id, backend_id.clone());
        state.sandboxes.insert(backend_id, entry);
        Ok(BackendLease::new(backend, instance))
    }

    pub async fn lease_bound_session(
        &self,
        session_id: &str,
    ) -> Result<BackendLease, SandboxError> {
        let state = self.state.lock().await;
        let backend_id =
            state
                .session_index
                .get(session_id)
                .ok_or_else(|| SandboxError::NotFound {
                    backend_id: format!("session:{session_id}"),
                })?;
        let entry = state
            .sandboxes
            .get(backend_id)
            .ok_or_else(|| SandboxError::NotFound {
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
            let Some(entry) = state.sandboxes.get_mut(&backend_id) else {
                return Ok(());
            };
            entry.session_ids.remove(session_id);
            if entry.session_ids.is_empty() {
                let removed = state.sandboxes.remove(&backend_id);
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
                .sandboxes
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
