use agent_contracts::backend::{
    BackendId, BackendLifecycleReason, BackendResourceLimits, OperationBackendBuildError,
    OperationError,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{
    backend_tree_node, build_backend, current_time_ms, delete_backend_instance, detach_from_parent,
    e2b, expires_at_ms_from_timeout, forked_provider_options, hash_config,
    load_global_max_sandbox_cnt, metadata_matches_filter, new_backend_id, requested_backend_id,
    resolve_backend_config, resolve_session_backend_config, workspace_root_string,
    BackendCheckoutRequest, BackendCheckoutResult, BackendCheckpointRef, BackendCheckpointRequest,
    BackendCheckpointResult, BackendCheckpointSnapshotDeleteRequest,
    BackendCheckpointSnapshotDeleteResult, BackendConnectRequest, BackendCreateRequest,
    BackendEnsureSessionRequest, BackendError, BackendForkRequest, BackendForkResult, BackendInfo,
    BackendInstanceEntry, BackendLease, BackendLineageEntry, BackendListFilter,
    BackendManagerLimits, BackendRegistry, BackendRegistryEntry, BackendTreeNode,
    BuildBackendInput, SandboxCounter, SandboxCounterKey, MAX_ACTIVE_SANDBOXES_PER_KEY,
    STALE_OWNER_THRESHOLD_MS,
};
use crate::gateway::SessionStore;

pub struct BackendManager {
    pub(super) state: Mutex<BackendManagerState>,
    limits: BackendManagerLimits,
    registry: BackendRegistry,
    sandbox_counter: SandboxCounter,
    process_id: String,
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
        Self::new_with_limits(BackendManagerLimits::default())
    }

    pub fn new_with_limits(limits: BackendManagerLimits) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self::new_with_storage_dir(limits, home.join(".xiaoo"))
    }

    /// Construct with an explicit storage directory for the shared registry
    /// and sandbox counter. Used by tests to isolate from the `~/.xiaoo/`
    /// files used by production so test runs never read or mutate a live
    /// daemon's state.
    pub fn new_with_storage_dir(limits: BackendManagerLimits, dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        let process_id = Self::generate_process_id();
        let max_per_key = limits.max_active_sandboxes_per_key.unwrap_or_else(|| {
            load_global_max_sandbox_cnt().unwrap_or(MAX_ACTIVE_SANDBOXES_PER_KEY)
        });
        let sandbox_counter = SandboxCounter::new_with_storage_dir(max_per_key, dir.clone());
        let registry = BackendRegistry::new_with_storage_dir(dir);
        Self {
            state: Mutex::new(BackendManagerState::default()),
            limits,
            registry,
            sandbox_counter,
            process_id,
        }
    }

    fn generate_process_id() -> String {
        let pid = std::process::id();
        let timestamp = current_time_ms();
        format!("{}_{}", pid, timestamp)
    }

    pub fn limits(&self) -> BackendManagerLimits {
        self.limits
    }

    pub fn registry(&self) -> &BackendRegistry {
        &self.registry
    }

    pub fn sandbox_counter(&self) -> &SandboxCounter {
        &self.sandbox_counter
    }

    /// Whether a backend of this `kind` participates in the shared sandbox
    /// counter / registry. Local backends are never counted.
    fn is_counted_kind(kind: &str) -> bool {
        kind == "e2b" || kind == "conch"
    }

    pub fn sandbox_key_from_config(config: &super::GatewayBackendConfig) -> SandboxCounterKey {
        if config.kind == "e2b" {
            // Use the same resolution path as the e2b provider so the
            // counter key matches the actual key used to authorise sandbox
            // creation (direct `api_key` field, then `api_key_env`, then
            // `E2B_API_KEY`). Falls back to a stable default only when no
            // key is configured at all.
            let key_identifier = e2b::resolve_api_key_from_options(&config.options)
                .unwrap_or_else(|| "e2b_default".to_string());
            SandboxCounterKey::new("e2b", key_identifier)
        } else if config.kind == "conch" {
            SandboxCounterKey::new("conch", "conch_default")
        } else {
            SandboxCounterKey::new("local", "local_default")
        }
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
        let sandbox_key = Self::sandbox_key_from_config(&config);

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

        let reserved_slot = if Self::is_counted_kind(&config.kind) {
            match self.sandbox_counter.check_and_reserve(&sandbox_key).await {
                Ok(_) => true,
                Err(super::SandboxCounterError::LimitExceeded { .. }) => {
                    return Err(BackendError::NeedsEviction);
                }
                Err(error) => return Err(error.into()),
            }
        } else {
            false
        };

        let entry = match build_backend(BuildBackendInput {
            backend_id: backend_id.clone(),
            config: config.clone(),
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
            e2b_bootstrap: None,
        })
        .await
        {
            Ok(entry) => entry,
            Err(error) => {
                if reserved_slot {
                    self.sandbox_counter
                        .cancel_reservation(&sandbox_key)
                        .await
                        .ok();
                }
                return Err(error);
            }
        };
        let info = entry.info();
        let session_ids = entry.session_ids.keys().cloned().collect::<Vec<_>>();
        for session_id in &session_ids {
            state
                .session_index
                .insert(session_id.clone(), backend_id.clone());
        }
        state.backends.insert(backend_id.clone(), entry);

        if Self::is_counted_kind(&config.kind) {
            if reserved_slot {
                self.sandbox_counter
                    .confirm_creation(&sandbox_key)
                    .await
                    .ok();
            }
            let registry_entry = BackendRegistryEntry::new(
                backend_id.0.clone(),
                sandbox_key,
                session_ids.clone(),
                self.process_id.clone(),
                info.instance_id.clone(),
            );
            self.registry.register(registry_entry).await.ok();
        }

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

    /// Release the shared counter slot, unregister from the shared registry,
    /// and delete the backend instance via its provider. Called after an
    /// entry has already been removed from the in-memory `state`. The
    /// counter release and registry unregister are no-ops for local
    /// backends (they were never registered).
    async fn finalize_removal(
        &self,
        instance: BackendInstanceEntry,
        reason: BackendLifecycleReason,
    ) -> Result<(), OperationError> {
        if Self::is_counted_kind(&instance.config.kind) {
            let sandbox_key = Self::sandbox_key_from_config(&instance.config);
            self.sandbox_counter.release(&sandbox_key).await.ok();
            self.registry
                .unregister(&instance.instance.backend_id.0)
                .await
                .ok();
        }
        delete_backend_instance(instance, reason).await
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

        self.finalize_removal(removed, BackendLifecycleReason::UserRequested)
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
        let mut create_config =
            super::GatewayBackendConfig::new("e2b", request.checkpoint.provider_options.clone());
        create_config.options = forked_provider_options(
            &request.checkpoint.provider_options,
            request.options.as_ref(),
            snapshot_id,
        );

        let sandbox_key = Self::sandbox_key_from_config(&create_config);

        let reserved_slot = {
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

            match self.sandbox_counter.check_and_reserve(&sandbox_key).await {
                Ok(_) => true,
                Err(super::SandboxCounterError::LimitExceeded { .. }) => {
                    return Err(BackendError::NeedsEviction);
                }
                Err(error) => return Err(error.into()),
            }
        };

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

        let mut child_entry = match build_backend(BuildBackendInput {
            backend_id: child_backend_id.clone(),
            config: create_config.clone(),
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
            e2b_bootstrap: None,
        })
        .await
        {
            Ok(entry) => entry,
            Err(error) => {
                if reserved_slot {
                    self.sandbox_counter
                        .cancel_reservation(&sandbox_key)
                        .await
                        .ok();
                }
                return Err(error);
            }
        };
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
            let cleanup_error = if let Some(entry) = child_entry {
                delete_backend_instance(entry, BackendLifecycleReason::UserRequested)
                    .await
                    .err()
            } else {
                None
            };
            if reserved_slot {
                self.sandbox_counter
                    .cancel_reservation(&sandbox_key)
                    .await
                    .ok();
            }
            if let Some(cleanup_error) = cleanup_error {
                return Err(BackendError::from_operation_error(cleanup_error));
            }
            return Err(error);
        }

        if reserved_slot {
            self.sandbox_counter
                .confirm_creation(&sandbox_key)
                .await
                .ok();
        }

        let registry_entry = BackendRegistryEntry::new(
            child_backend_id.0.clone(),
            sandbox_key,
            child_session_id.iter().cloned().collect(),
            self.process_id.clone(),
            child_info.instance_id.clone(),
        );
        self.registry.register(registry_entry).await.ok();

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
        session_store: Arc<dyn SessionStore>,
    ) -> Result<BackendLease, OperationBackendBuildError> {
        let config = resolve_session_backend_config(request.config.clone())?;
        let workspace_root = workspace_root_string(&request.workspace_root)?;
        let config_hash = hash_config(&config);
        let sandbox_key = Self::sandbox_key_from_config(&config);
        let needs_counting = Self::is_counted_kind(&config.kind);

        // Fast path: if the session already has a live backend in the
        // in-memory state, return it without reserving a sandbox slot.
        // This is critical because `reserve_with_eviction` checks the
        // shared counter BEFORE the existing-binding check inside the state
        // lock — so without this fast path, a session that already owns one
        // of the MAX_ACTIVE_SANDBOXES_PER_KEY slots would fail with "limit
        // reached" on its second turn instead of reusing its backend.
        {
            let state = self.state.lock().await;
            if let Some(existing_backend_id) = state.session_index.get(&request.session_id) {
                if let Some(entry) = state.backends.get(existing_backend_id) {
                    if entry.workspace_root == workspace_root && entry.config_hash == config_hash {
                        self.registry
                            .update_activity(&existing_backend_id.0)
                            .await
                            .ok();
                        return Ok(BackendLease::new(
                            Arc::clone(&entry.backend),
                            entry.instance.clone(),
                        ));
                    }
                }
            }
        }

        // Reserve a sandbox slot with eviction-driven retry. This must happen
        // WITHOUT holding the in-memory `state` lock, because evicting an idle
        // backend needs to acquire that same lock.
        if needs_counting {
            self.reserve_with_eviction(&sandbox_key, &config.options, session_store.clone())
                .await
                .map_err(BackendError::into_build_error)?;
        }

        let mut state = self.state.lock().await;

        // Re-check for an existing binding once we hold the lock; a concurrent
        // ensure_session_backend may have created one while we were reserving.
        if let Some(existing_backend_id) = state.session_index.get(&request.session_id) {
            let entry = match state.backends.get(existing_backend_id) {
                Some(entry) => entry,
                None => {
                    // The session was bound to a backend that has since been
                    // evicted/removed but the session_index mapping was not
                    // cleaned up. Cancel the reservation we just made so the
                    // sandbox slot is not leaked; the caller will surface the
                    // error and the session can be retried/resumed.
                    let missing_backend_id = existing_backend_id.0.clone();
                    drop(state);
                    if needs_counting {
                        self.sandbox_counter
                            .cancel_reservation(&sandbox_key)
                            .await
                            .ok();
                    }
                    return Err(OperationBackendBuildError::BuildFailed {
                        message: format!(
                            "session {} is bound to missing backend {}",
                            request.session_id, missing_backend_id
                        ),
                    });
                }
            };
            if entry.workspace_root != workspace_root || entry.config_hash != config_hash {
                if needs_counting {
                    self.sandbox_counter
                        .cancel_reservation(&sandbox_key)
                        .await
                        .ok();
                }
                return Err(OperationBackendBuildError::BuildFailed {
                    message: format!(
                        "session {} is already bound to backend {} with different workspace or config",
                        request.session_id, existing_backend_id
                    ),
                });
            }
            if needs_counting {
                self.sandbox_counter
                    .cancel_reservation(&sandbox_key)
                    .await
                    .ok();
            }
            self.registry
                .update_activity(&existing_backend_id.0)
                .await
                .ok();
            return Ok(BackendLease::new(
                Arc::clone(&entry.backend),
                entry.instance.clone(),
            ));
        }

        let backend_id = new_backend_id();
        let entry = match build_backend(BuildBackendInput {
            backend_id: backend_id.clone(),
            config: config.clone(),
            workspace_root_text: workspace_root,
            config_hash,
            session_id_for_instance: request.session_id.clone(),
            session_id: Some(request.session_id.clone()),
            resource_limits: BackendResourceLimits::default(),
            metadata: Value::Null,
            expires_at_ms: None,
            lineage: BackendLineageEntry::default(),
            backend_checkpoint: None,
            e2b_bootstrap: request.e2b_bootstrap,
        })
        .await
        {
            Ok(entry) => entry,
            Err(error) => {
                if needs_counting {
                    self.sandbox_counter
                        .cancel_reservation(&sandbox_key)
                        .await
                        .ok();
                }
                return Err(error.into_build_error());
            }
        };
        let backend = Arc::clone(&entry.backend);
        let instance = entry.instance.clone();

        state
            .session_index
            .insert(request.session_id.clone(), backend_id.clone());
        state.backends.insert(backend_id.clone(), entry);

        if needs_counting {
            self.sandbox_counter
                .confirm_creation(&sandbox_key)
                .await
                .ok();
            let registry_entry = BackendRegistryEntry::new(
                backend_id.0.clone(),
                sandbox_key,
                vec![request.session_id.clone()],
                self.process_id.clone(),
                instance.instance_id.0.clone(),
            );
            self.registry.register(registry_entry).await.ok();
        }

        Ok(BackendLease::new(backend, instance))
    }

    /// Reserve a sandbox slot for `sandbox_key`, evicting idle sandboxes (and
    /// briefly waiting for cross-process evictions) when the pool is full.
    /// `provider_options` is the requesting process's own options for this
    /// sandbox key; it is used to authorise remote deletion of orphaned e2b
    /// sandboxes (same api_key, since the sandbox_key matches).
    async fn reserve_with_eviction(
        &self,
        sandbox_key: &SandboxCounterKey,
        provider_options: &serde_json::Value,
        session_store: Arc<dyn SessionStore>,
    ) -> Result<(), BackendError> {
        let max_attempts = self.limits.max_eviction_attempts.max(1);
        // Track the remote backend (if any) that *this* call marked for
        // eviction. If we exhaust the retry budget without securing a slot,
        // we clear the mark so the next attempt starts from a clean slate
        // and re-targets the earliest idle sandbox — rather than leaving a
        // lingering mark that would make the next attempt skip it and mark a
        // different (later) sandbox.
        let mut marked_backend_id: Option<String> = None;
        for _ in 0..max_attempts {
            match self.sandbox_counter.check_and_reserve(sandbox_key).await {
                Ok(_) => {
                    // Success: a previously-set mark (if any) is no longer
                    // our concern — the marked backend's owner may still
                    // evict it, but that is the normal "one extra slot
                    // freed" case and the owner will skip eviction if the
                    // sandbox became active again.
                    return Ok(());
                }
                Err(super::SandboxCounterError::LimitExceeded { .. }) => {}
                Err(error) => return Err(error.into()),
            }
            // Pool full: try to free a slot by evicting an idle sandbox owned
            // by this process, reclaiming an orphaned sandbox whose owner died,
            // or by asking another (live) process to evict its idle sandbox.
            match self
                .try_evict_if_needed(sandbox_key, provider_options, session_store.clone())
                .await
            {
                Ok(None) => {
                    // A sandbox was evicted directly (own or orphan). Clear
                    // any mark we previously set: we no longer need a remote
                    // eviction, and leaving the mark would cause an
                    // unnecessary eviction of an idle sandbox.
                    if let Some(id) = marked_backend_id.take() {
                        self.registry.set_pending_eviction(&id, false).await.ok();
                    }
                    continue;
                }
                Ok(Some(id)) => {
                    // A remote sandbox was marked for eviction. Record it so
                    // we can roll back on give-up, then wait for its owner to
                    // evict it before retrying.
                    marked_backend_id = Some(id);
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
                Err(BackendError::NeedsEviction) => {
                    // Either no evictable sandbox was found, or another
                    // backend is already marked for eviction (by us or
                    // another process). Wait briefly for the mark to be
                    // resolved and retry.
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        // Retry budget exhausted. Roll back any mark this call set so the
        // final state is equivalent to "no deletable sandbox found, just an
        // error" — leaving `pending_eviction` set would make the next attempt
        // skip the earliest idle sandbox and target a later one instead.
        if let Some(id) = marked_backend_id.take() {
            self.registry.set_pending_eviction(&id, false).await.ok();
        }
        match self.sandbox_counter.check_and_reserve(sandbox_key).await {
            Ok(_) => Ok(()),
            Err(super::SandboxCounterError::LimitExceeded { .. }) => {
                Err(BackendError::ResourceLimitExceeded {
                    message: "sandbox limit reached; no idle sandbox available to evict"
                        .to_string(),
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Attempt to free a sandbox slot by evicting an idle sandbox.
    ///
    /// Returns:
    /// - `Ok(None)` when a sandbox owned by this process (or an orphaned
    ///   sandbox whose owner died) was evicted directly — a slot is freed
    ///   and the caller should retry `check_and_reserve` immediately.
    /// - `Ok(Some(backend_id))` when a *remote* (other-process, live owner)
    ///   sandbox was marked for eviction (`pending_eviction=true`) and
    ///   signalled via SIGUSR1. The caller should wait briefly for its owner
    ///   to perform the eviction and then retry. The returned id lets the
    ///   caller roll back the mark if it ultimately gives up.
    /// - `Err(NeedsEviction)` when no sandbox was evicted or marked — either
    ///   because nothing is evictable, or because another backend for this
    ///   key is *already* marked for eviction (by this process or another).
    ///   In the latter case we deliberately do not mark a second one: a
    ///   single pending mark is enough to free one slot, and marking more
    ///   would cause every signalled owner to evict its sandbox in parallel,
    ///   freeing multiple slots when only one is needed.
    pub async fn try_evict_if_needed(
        &self,
        sandbox_key: &SandboxCounterKey,
        provider_options: &serde_json::Value,
        session_store: Arc<dyn SessionStore>,
    ) -> Result<Option<String>, BackendError> {
        let total_count = self.sandbox_counter.get_total_count(sandbox_key).await?;
        if total_count < self.sandbox_counter.max_per_key() {
            return Ok(None);
        }

        let mut entries = self.registry.get_entries_by_key(sandbox_key).await?;
        // Evict the earliest-created evictable sandbox first.
        entries.sort_by_key(|entry| entry.created_at_ms);

        // Already-marked guard: at most one backend per key is marked at a
        // time (see method doc comment).
        let already_marked = entries.iter().any(|entry| entry.pending_eviction);
        if already_marked {
            return Err(BackendError::NeedsEviction);
        }

        for entry in entries {
            // Eviction eligibility is based on the shared BackendRegistry
            // session-status snapshot (written by every process on idle
            // transitions) plus the owner heartbeat (for detecting dead
            // processes), so it works uniformly for own- and other-process
            // backends.
            if !entry.is_evictable(STALE_OWNER_THRESHOLD_MS) {
                continue;
            }

            if entry.owner_process_id == self.process_id {
                let checkpoint = if self.limits.checkpoint_before_eviction {
                    self.create_checkpoint_for_eviction(&entry.backend_id).await
                } else {
                    None
                };

                match self.evict_backend_by_id(&entry.backend_id).await {
                    Ok(()) => {}
                    Err(BackendError::NotFound { .. }) => {
                        // Another thread in this process already evicted
                        // this backend. Skip to the next entry.
                        continue;
                    }
                    Err(error) => return Err(error),
                }

                let session_ids = entry.session_ids.clone();
                session_store
                    .batch_mark_paused_due_to_eviction(&session_ids, checkpoint)
                    .await;

                return Ok(None);
            } else if entry.is_owner_stale(STALE_OWNER_THRESHOLD_MS) {
                // The owner process is presumed dead. Reclaim the sandbox
                // directly: release the counter, unregister, and best-effort
                // delete the remote e2b sandbox. We cannot checkpoint a dead
                // owner's sandbox through the normal path, so the bound
                // sessions (which lived in the dead process's memory) are
                // lost — but the sandbox slot is freed for the new task.
                self.reclaim_orphaned_backend(&entry, provider_options)
                    .await?;
                return Ok(None);
            } else {
                // Ask the live owner process to evict its idle sandbox.
                // Set the pending flag in the shared registry and send
                // SIGUSR1 so the owner wakes up immediately and performs
                // the eviction in its signal handler.
                //
                // Because of the `already_marked` guard above, at most one
                // backend per key is ever marked at a time. The caller must
                // roll back this mark (clear `pending_eviction`) if it gives
                // up waiting, so the next attempt can re-mark the earliest
                // idle sandbox and re-signal the owner.
                self.registry
                    .set_pending_eviction(&entry.backend_id, true)
                    .await
                    .ok();
                if entry.owner_pid != 0 && entry.owner_pid != std::process::id() {
                    // Ignore errors — the owner may have died between our
                    // heartbeat check and the signal send; the orphan-
                    // reclaim path handles that on the next retry.
                    unsafe { libc::kill(entry.owner_pid as i32, libc::SIGUSR1) };
                }
                return Ok(Some(entry.backend_id.clone()));
            }
        }

        // No own idle sandbox was evicted, no orphaned sandbox was reclaimed,
        // and no remote sandbox was marked — nothing was evictable at all.
        Err(BackendError::NeedsEviction)
    }

    /// Reclaim a backend whose owner process has died. Any process can
    /// perform this: release the shared counter, remove the registry entry,
    /// and best-effort delete the remote sandbox.
    async fn reclaim_orphaned_backend(
        &self,
        entry: &BackendRegistryEntry,
        provider_options: &serde_json::Value,
    ) -> Result<(), BackendError> {
        tracing::info!(
            backend_id = %entry.backend_id,
            instance_id = %entry.instance_id,
            owner = %entry.owner_process_id,
            "Reclaiming orphaned backend (owner process presumed dead)"
        );

        self.sandbox_counter.release(&entry.sandbox_key).await.ok();
        self.registry.unregister(&entry.backend_id).await.ok();

        if entry.sandbox_key.sandbox_type == "e2b" && !entry.instance_id.is_empty() {
            if let Err(error) = e2b::delete_sandbox_by_id(e2b::E2bDeleteSandboxInput {
                provider_options: provider_options.clone(),
                sandbox_id: entry.instance_id.clone(),
            })
            .await
            {
                tracing::warn!(
                    backend_id = %entry.backend_id,
                    instance_id = %entry.instance_id,
                    error = %error,
                    "Failed to delete orphaned e2b sandbox; slot is freed but sandbox may linger"
                );
            }
        }

        Ok(())
    }

    async fn create_checkpoint_for_eviction(
        &self,
        backend_id: &str,
    ) -> Option<super::BackendCheckpointRef> {
        let result = self
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: Some(backend_id.to_string()),
                session_id: None,
                name: Some(format!("eviction-{}", backend_id)),
                metadata: serde_json::json!({"reason": "eviction"}),
            })
            .await;

        match result {
            Ok(checkpoint_result) => {
                let checkpoint = checkpoint_result.checkpoint.clone();
                if let Some(snapshot_id) = checkpoint.provider_snapshot_id.as_ref() {
                    tracing::info!(
                        backend_id = backend_id,
                        checkpoint_id = %checkpoint.checkpoint_id,
                        snapshot_id = %snapshot_id,
                        "Created checkpoint for eviction"
                    );
                }
                Some(checkpoint)
            }
            Err(e) => {
                tracing::warn!(
                    backend_id = backend_id,
                    error = %e,
                    "Failed to create checkpoint for eviction"
                );
                None
            }
        }
    }

    pub async fn evict_backend_by_id(&self, backend_id: &str) -> Result<(), BackendError> {
        let removed =
            {
                let mut state = self.state.lock().await;
                let backend_id_obj = BackendId(backend_id.to_string());
                let entry = state.backends.remove(&backend_id_obj).ok_or_else(|| {
                    BackendError::NotFound {
                        backend_id: backend_id.to_string(),
                    }
                })?;

                for session_id in entry.session_ids.keys() {
                    state.session_index.remove(session_id);
                }
                detach_from_parent(&mut state, &backend_id_obj, &entry);
                entry
            };

        self.finalize_removal(removed, BackendLifecycleReason::ManagerReconcile)
            .await
            .map_err(BackendError::from_operation_error)?;

        tracing::info!(backend_id = backend_id, "Backend evicted successfully");
        Ok(())
    }

    /// Clear the `pending_eviction` mark on a backend in the shared registry.
    ///
    /// Used by callers of [`try_evict_if_needed`] that received
    /// `Ok(Some(backend_id))` (a remote sandbox was marked for eviction) but
    /// later gave up waiting for the eviction to complete. Rolling back the
    /// mark restores the backend to the evictable pool so the next attempt
    /// re-targets the earliest idle sandbox instead of skipping it.
    ///
    /// This is best-effort: if the backend was already evicted (or never
    /// existed) the call silently does nothing.
    pub async fn clear_pending_eviction(&self, backend_id: &str) {
        self.registry
            .set_pending_eviction(backend_id, false)
            .await
            .ok();
    }

    /// Evict this process's own backends that another process has marked for
    /// eviction (and are idle according to the shared registry snapshot).
    pub async fn check_and_evict_marked_backends(
        &self,
        session_store: Arc<dyn SessionStore>,
    ) -> Result<(), BackendError> {
        // Refresh heartbeats for our own backends first, so other processes
        // know we are alive and don't try to reclaim them as orphans.
        self.registry
            .refresh_heartbeats_for_process(&self.process_id)
            .await?;

        let entries = self
            .registry
            .get_entries_for_process(&self.process_id)
            .await?;

        for entry in entries {
            if !entry.pending_eviction {
                continue;
            }
            // The entry is already marked for eviction; only confirm the
            // backend is still in a safe state (idle or owner stale) to
            // delete. Using `is_evictable` here would always return false
            // because `pending_eviction` is true, causing the handler to
            // clear the mark and skip — and the requesting process would
            // never see the slot freed.
            if !entry.is_eviction_safe(STALE_OWNER_THRESHOLD_MS) {
                // The backend became active again; clear the stale mark.
                self.registry
                    .set_pending_eviction(&entry.backend_id, false)
                    .await
                    .ok();
                continue;
            }

            tracing::info!(
                backend_id = %entry.backend_id,
                "Evicting backend marked for eviction by another process"
            );
            let checkpoint = if self.limits.checkpoint_before_eviction {
                self.create_checkpoint_for_eviction(&entry.backend_id).await
            } else {
                None
            };
            self.evict_backend_by_id(&entry.backend_id).await?;
            session_store
                .batch_mark_paused_due_to_eviction(&entry.session_ids, checkpoint)
                .await;
        }

        Ok(())
    }

    /// Reconcile the persisted sandbox counter with the shared registry.
    ///
    /// The counter file (`~/.xiaoo/sandbox_counts.json`) is persisted across
    /// process restarts, so a previous process that exited or crashed without
    /// releasing its sandboxes leaves "zombie" counts behind. This method
    /// recomputes the count per sandbox key from ALL registry entries (both
    /// live and stale-owner) and resets the counter to match.
    ///
    /// Counting stale-owner entries too is essential: the orphan-reclaim
    /// path in `reclaim_orphaned_backend` calls `release` on the counter when
    /// it deletes a stale entry, so the counter must include that entry for
    /// the decrement to be correct. If stale entries were excluded here, the
    /// counter would under-count, and each orphan reclaim would decrement a
    /// slot that belongs to a different (live) sandbox — effectively letting
    /// `MAX_ACTIVE_SANDBOXES_PER_KEY + N_stale` sandboxes coexist before the
    /// limit truly bites.
    ///
    /// Should be called once on startup before any new sandbox is created.
    pub async fn reconcile_counts_with_registry(&self) -> Result<(), BackendError> {
        let entries = self.registry.get_all_entries().await?;
        let mut live_counts: HashMap<String, usize> = HashMap::new();
        for entry in entries {
            // Count every registry entry, including those whose owner
            // heartbeat is stale. A stale-owner backend still occupies a
            // remote sandbox slot until it is reclaimed via
            // `reclaim_orphaned_backend`, which will `release` the counter
            // at that point — so it must be counted here for the eventual
            // decrement to balance correctly.
            *live_counts
                .entry(entry.sandbox_key.to_string_key())
                .or_default() += 1;
        }
        self.sandbox_counter.reconcile_counts(live_counts).await?;
        Ok(())
    }

    /// Start a background task that refreshes this process's owner heartbeats
    /// every few seconds (so other processes know we are alive) and listens on
    /// SIGUSR1 for cross-process eviction requests from other xiaoo-daemon /
    /// xiaoo processes. When SIGUSR1 arrives the task evicts any of our
    /// backends that another process has marked with `pending_eviction=true`.
    ///
    /// Falls back to a 3-second polling loop on platforms where SIGUSR1
    /// registration fails.
    pub fn start_signal_handler(
        self: Arc<Self>,
        session_store: Arc<dyn SessionStore>,
    ) -> tokio::task::JoinHandle<()> {
        use tokio::signal::unix::{signal, SignalKind};

        tokio::spawn(async move {
            // Reconcile the persisted sandbox counter with the registry on
            // startup, purging zombie counts left by previous processes that
            // exited without releasing their sandboxes.
            if let Err(error) = self.reconcile_counts_with_registry().await {
                tracing::warn!(error = %error, "startup sandbox counter reconciliation failed");
            }

            let mut sigusr1 = match signal(SignalKind::user_defined1()) {
                Ok(sig) => Some(sig),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        "SIGUSR1 unavailable; falling back to 3s polling for cross-process eviction"
                    );
                    None
                }
            };

            let mut heartbeat_tick = tokio::time::interval(tokio::time::Duration::from_secs(3));
            // Skip the first immediate tick; we just registered a fresh
            // heartbeat during backend creation.
            heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                tokio::select! {
                    _ = heartbeat_tick.tick() => {
                        if let Err(error) = self
                            .registry()
                            .refresh_heartbeats_for_process(&self.process_id)
                            .await
                        {
                            tracing::warn!(error = %error, "heartbeat refresh failed");
                        }
                    }
                    _ = async {
                        match sigusr1.as_mut() {
                            Some(sig) => sig.recv().await,
                            // No SIGUSR1 → fall back to a 3s poll for
                            // pending evictions.
                            None => {
                                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                Some(())
                            }
                        }
                    } => {
                        if let Err(error) = self
                            .check_and_evict_marked_backends(session_store.clone())
                            .await
                        {
                            tracing::warn!(error = %error, "eviction handler failed");
                        }
                    }
                }
            }
        })
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
            self.finalize_removal(instance, BackendLifecycleReason::SessionClose)
                .await?;
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
            self.finalize_removal(instance, BackendLifecycleReason::DaemonShutdown)
                .await?;
        }
        Ok(())
    }
}
