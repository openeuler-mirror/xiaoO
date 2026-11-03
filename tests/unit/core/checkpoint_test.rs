use super::*;
use crate::LoopState;
use agent_llm::ChatMessageExt;
use agent_types::{ChatMessage, MessageRole};
use std::sync::Arc;

#[test]
fn memory_cached_sqlite_config_is_supported_but_storage_is_not_implemented() {
    let config: CheckpointConfig =
        serde_json::from_value(serde_json::json!({"storage": {"kind": "memory_cached_sqlite"}}))
            .unwrap();
    assert_eq!(config.storage.kind, StorageKind::MemoryCachedSqlite);
    assert!(matches!(
        create(config),
        Err(CheckpointError::NotImplemented("MemoryCachedSqliteStorage"))
    ));
    assert_eq!(
        CheckpointConfig::default().storage.kind,
        StorageKind::Memory
    );
    assert!(create(CheckpointConfig::default()).is_ok());
}

#[cfg(unix)]
fn socket_config() -> CheckpointConfig {
    CheckpointConfig {
        enabled: true,
        unix_socket: Some(UnixSocketConfig {
            endpoint: std::env::temp_dir()
                .join(format!("xiaoo-test-{}.sock", uuid::Uuid::new_v4())),
        }),
        ..Default::default()
    }
}

fn push(state: &mut LoopState, text: &str) {
    state
        .messages
        .write()
        .push(ChatMessage::text(MessageRole::User, text, 0));
}
async fn setup() -> (Arc<dyn CheckpointBackend>, CheckpointDriver, LoopState) {
    let backend = create(Default::default()).unwrap();
    let session_id = uuid::Uuid::new_v4().to_string();
    let binding = backend
        .init(
            SnapshotContext {
                session_id: session_id.clone(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap();
    (backend, binding.driver.unwrap(), LoopState::new(session_id))
}

#[tokio::test]
async fn snapshot_readiness_and_restore_preserve_shared_history_and_cancel() {
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    push(&mut state, "base");
    state.plan_nudged = true;
    state.repeated_failure_count = 3;
    state.last_failure_sig = Some(7);
    state.turn_count = 8;
    let shared_messages = state.messages_arc();
    let cancel = state.cancel.clone();
    let mut turn = 4;
    let snapshot = backend.save(context.clone()).unwrap();
    assert!(matches!(
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    mode: RestoreMode::EnsureSafeMode,
                    snapshot_id: snapshot.id.clone()
                }
            )
            .await,
        Err(CheckpointError::SnapshotNotReadyOrNotFound(_))
    ));
    driver.process_pending(&mut state, &mut turn, false).await;
    push(&mut state, "future");
    state.plan_nudged = false;
    state.repeated_failure_count = 0;
    state.turn_count = 11;
    turn = 7;
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: snapshot.id,
            },
        )
        .await
        .unwrap();
    assert_eq!(shared_messages.read().len(), 2, "restore only enqueues");
    driver.process_pending(&mut state, &mut turn, false).await;
    assert!(Arc::ptr_eq(&shared_messages, &state.messages));
    assert_eq!(shared_messages.read().len(), 1);
    assert!(state.plan_nudged);
    assert_eq!(state.repeated_failure_count, 3);
    assert_eq!(state.last_failure_sig, Some(7));
    assert_eq!(state.turn_count, 8);
    assert_eq!(turn, 4);
    cancel.cancel();
    assert!(state.cancel.is_cancelled());
}

#[tokio::test]
async fn restore_then_delete_executes_in_order_and_delete_rejects_new_restores() {
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    push(&mut state, "base");
    let mut turn = 1;
    let a = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    push(&mut state, "future");
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: a.id.clone(),
            },
        )
        .await
        .unwrap();
    backend
        .delete_snapshot(context.clone(), &a.id)
        .await
        .unwrap();
    assert!(backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: a.id.clone()
            }
        )
        .await
        .is_err());
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(state.messages.read().len(), 1);
    assert!(backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: a.id
            }
        )
        .await
        .is_err());
}

#[tokio::test]
async fn one_boundary_keeps_independent_snapshots_in_command_order() {
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    push(&mut state, "A");
    let mut turn = 1;
    let a = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    push(&mut state, "B");
    let b = backend.save(context.clone()).unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: a.id.clone(),
            },
        )
        .await
        .unwrap();
    let c = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    backend
        .delete_snapshot(context.clone(), &a.id)
        .await
        .unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: b.id,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(state.messages.read().len(), 2);
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: c.id,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(
        state.messages.read().len(),
        1,
        "C owns its data after A is deleted"
    );
}

#[tokio::test]
async fn release_reinit_reuses_driver_and_preserves_accepted_commands() {
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    let mut turn = 1;
    push(&mut state, "base");
    let a = backend.save(context.clone()).unwrap();
    backend.release(context.clone());
    backend.release(context.clone()); // Releasing an already released handle is a no-op.
    assert!(matches!(
        backend.save(context.clone()),
        Err(CheckpointError::Released)
    ));
    assert!(matches!(
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    mode: RestoreMode::EnsureSafeMode,
                    snapshot_id: a.id.clone()
                }
            )
            .await,
        Err(CheckpointError::Released)
    ));
    assert!(matches!(
        backend.delete_snapshot(context.clone(), &a.id).await,
        Err(CheckpointError::Released)
    ));
    assert!(!driver.is_hot().await);
    driver.process_pending(&mut state, &mut turn, false).await;
    assert!(
        !driver.is_hot().await,
        "queued snapshot runs before cooling"
    );
    assert!(matches!(
        backend
            .init(
                SnapshotContext {
                    session_id: "another-session".into()
                },
                serde_json::json!({})
            )
            .await,
        Err(CheckpointError::SessionMismatch)
    ));
    let binding = backend
        .init(
            SnapshotContext {
                session_id: state.session_id.clone(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert!(
        binding.driver.is_none(),
        "re-init must reuse the agent's driver"
    );
    assert!(matches!(
        backend
            .init(
                SnapshotContext {
                    session_id: state.session_id.clone()
                },
                serde_json::json!({})
            )
            .await,
        Err(CheckpointError::AlreadyInitialized)
    ));
    push(&mut state, "future");
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: a.id.clone(),
            },
        )
        .await
        .unwrap();
    backend
        .delete_snapshot(context.clone(), &a.id)
        .await
        .unwrap();
    backend.release(context.clone());
    // Re-init BEFORE the prior Release reaches the driver.
    assert!(backend
        .init(
            SnapshotContext {
                session_id: state.session_id.clone()
            },
            serde_json::json!({})
        )
        .await
        .unwrap()
        .driver
        .is_none());
    assert!(
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    mode: RestoreMode::EnsureSafeMode,
                    snapshot_id: a.id.clone()
                }
            )
            .await
            .is_err(),
        "logical deletion survives re-init"
    );
    let b = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert!(
        driver.is_hot().await,
        "old Release cannot cool the new activation"
    );
    assert_eq!(
        state.messages.read().len(),
        1,
        "accepted restore survives release"
    );
    assert!(backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: a.id
            }
        )
        .await
        .is_err());
    push(&mut state, "later");
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: b.id,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(state.messages.read().len(), 1);
    drop(driver);
    assert!(matches!(
        backend.save(context.clone()),
        Err(CheckpointError::QueueClosed)
    ));
}

#[tokio::test]
async fn checkpoint_serialization_and_plan_restore() {
    let (_, _, mut state) = setup().await;
    let plan = serde_json::from_value(
        serde_json::json!([{"content":"original plan", "status":"pending"}]),
    )
    .unwrap();
    tool::restore_checkpoint_todos(&state.session_id, plan);
    let context = state::ContextState::capture(&state, 3);
    let context: state::ContextState =
        serde_json::from_slice(&serde_json::to_vec(&context).unwrap()).unwrap();
    tool::restore_checkpoint_todos(&state.session_id, Vec::new());
    let mut turn = 8;
    context.apply(&mut state, &mut turn);
    assert_eq!(turn, 3);
    assert_eq!(
        tool::checkpoint_todos(&state.session_id)[0].content,
        "original plan"
    );
    tool::restore_checkpoint_todos(&state.session_id, Vec::new());
}

#[cfg(unix)]
#[tokio::test]
async fn unix_socket_routes_requests_and_reactivates_same_driver() {
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixStream,
    };
    async fn call(path: &std::path::Path, request: serde_json::Value) -> serde_json::Value {
        let mut stream = UnixStream::connect(path).await.unwrap();
        stream
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .await
            .unwrap();
        serde_json::from_str(&response).unwrap()
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    let context = SnapshotContext {
        session_id: session_id.clone(),
    };
    let endpoint = UnixCheckpointEndpoint::bind(session_id.clone(), socket_config())
        .await
        .unwrap();
    let path = endpoint.path().to_owned();
    assert!(matches!(
        endpoint.backend.save(context.clone()),
        Err(CheckpointError::NotInitialized)
    ));
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"save", "context":{"session_id":session_id}})
        )
        .await["ok"],
        false
    );
    let initialized = call(
        &path,
        serde_json::json!({"op":"init", "context":{"session_id":session_id}}),
    )
    .await;
    assert_eq!(initialized["ok"], true);
    assert_eq!(initialized["result"]["session_id"], session_id);
    // No LoopState or user input was needed to initialize the socket backend.
    let mut state = LoopState::new(session_id.clone());
    push(&mut state, "before");
    let mut turn = 1;
    let snapshot = call(
        &path,
        serde_json::json!({"op":"save", "context":{"session_id":session_id}}),
    )
    .await;
    assert_eq!(snapshot["ok"], true);
    let id = snapshot["result"]["id"].clone();
    let missing_mode = call(
        &path,
        serde_json::json!({"op":"restore", "context":{"session_id":session_id}, "snapshot_id":id}),
    )
    .await;
    assert_eq!(missing_mode["ok"], false);
    assert!(missing_mode["error"].as_str().unwrap().contains("mode"));
    // The same Pending ID is accepted only by the explicit promise mode.
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"restore", "context":{"session_id":session_id}, "mode":"promise_mode", "snapshot_id":id})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"restore", "context":{"session_id":session_id}, "mode":"ensure_safe_mode", "snapshot_id":id})
        )
        .await["ok"],
        false
    );
    endpoint
        .driver
        .process_pending(&mut state, &mut turn, false)
        .await;
    push(&mut state, "after");
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"restore", "context":{"session_id":session_id}, "mode":"ensure_safe_mode", "snapshot_id":id})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"delete_snapshot", "context":{"session_id":session_id}, "snapshot_id":id})
        )
        .await["ok"],
        true
    );
    endpoint
        .driver
        .process_pending(&mut state, &mut turn, false)
        .await;
    assert_eq!(state.messages.read().len(), 1);
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"release", "context":{"session_id":session_id}})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"save", "context":{"session_id":session_id}})
        )
        .await["ok"],
        false
    );
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"init", "context":{"session_id":session_id}})
        )
        .await["ok"],
        true
    );
    let snapshot = call(
        &path,
        serde_json::json!({"op":"save", "context":{"session_id":session_id}}),
    )
    .await;
    endpoint
        .driver
        .process_pending(&mut state, &mut turn, false)
        .await;
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"restore", "context":{"session_id":session_id}, "mode":"ensure_safe_mode", "snapshot_id":snapshot["result"]["id"]})
        )
        .await["ok"],
        true
    );
    // Release/re-init queue lifecycle events without replacing the driver.
    let original_driver = endpoint.driver.clone();
    let pending = call(
        &path,
        serde_json::json!({"op":"save", "context":{"session_id":session_id}}),
    )
    .await;
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"release", "context":{"session_id":session_id}})
        )
        .await["ok"],
        true
    );
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        call(
            &path,
            serde_json::json!({"op":"init", "context":{"session_id":session_id}}),
        ),
    )
    .await
    .unwrap();
    assert_eq!(response["ok"], true);
    endpoint
        .driver
        .process_pending(&mut state, &mut turn, false)
        .await;
    assert!(endpoint.driver.is_hot().await);
    assert!(Arc::ptr_eq(&original_driver, &endpoint.driver));
    drop(original_driver);
    assert_eq!(
        call(
            &path,
            serde_json::json!({"op":"restore", "context":{"session_id":session_id}, "mode":"ensure_safe_mode", "snapshot_id":pending["result"]["id"]})
        )
        .await["ok"],
        true
    );
    drop(endpoint);
    assert!(!path.exists());
}

#[tokio::test]
async fn failed_save_does_not_make_snapshot_restorable() {
    use super::{
        backend::QueuedCheckpointBackend, state::StoredCheckpoint, storage::CheckpointStorage,
    };
    use async_trait::async_trait;
    struct FailedStorage;
    #[async_trait]
    impl CheckpointStorage for FailedStorage {
        fn close(&self) {}
        async fn save(&self, _: StoredCheckpoint) -> Result<Option<SnapshotId>, CheckpointError> {
            Err(CheckpointError::Storage("save failed".into()))
        }
        async fn load(&self, _: &SnapshotId) -> Result<Option<StoredCheckpoint>, CheckpointError> {
            Ok(None)
        }
        async fn delete(&self, _: &SnapshotId) -> Result<(), CheckpointError> {
            Ok(())
        }
    }
    let context = SnapshotContext {
        session_id: "failed-save".into(),
    };
    let backend = QueuedCheckpointBackend::new(
        Arc::new(FailedStorage),
        CheckpointConfig {
            save_mode: SaveMode::Next,
            ..Default::default()
        },
    );
    let driver = backend
        .init(
            SnapshotContext {
                session_id: "failed-save".into(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap()
        .driver
        .unwrap();
    let snapshot = backend.save(context.clone()).unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: snapshot.id.clone(),
                mode: RestoreMode::PromiseMode,
            },
        )
        .await
        .unwrap();
    let mut state = LoopState::new("failed-save".into());
    push(&mut state, "unchanged");
    driver.process_pending(&mut state, &mut 1, false).await;
    assert!(backend
        .list_checkpoint(
            SnapshotContext {
                session_id: "failed-save".into()
            },
            AccessMode::LastN,
            None,
        )
        .unwrap()
        .is_empty());
    for mode in [RestoreMode::EnsureSafeMode, RestoreMode::PromiseMode] {
        assert!(matches!(
            backend
                .restore(
                    context.clone(),
                    RestoreRequest {
                        mode,
                        snapshot_id: snapshot.id.clone()
                    }
                )
                .await,
            Err(CheckpointError::SnapshotFailed(_))
        ));
    }
    // The promise accepted before the failed save also fails explicitly when
    // its queued restore reaches the driver; history is never replaced.
    driver.process_pending(&mut state, &mut 1, false).await;
    assert_eq!(state.messages.read().len(), 1);
    assert_eq!(state.messages.read()[0].text_content(), Some("unchanged"));
}

#[tokio::test]
async fn release_barrier_keeps_pending_save_and_physical_delete() {
    use super::{backend::QueuedCheckpointBackend, storage::StorageFactory};
    let storage = storage::factory().create(StorageConfig::default()).unwrap();
    let backend = QueuedCheckpointBackend::new(
        storage.clone(),
        CheckpointConfig {
            save_mode: SaveMode::Next,
            ..Default::default()
        },
    );
    let driver = backend
        .init(
            SnapshotContext {
                session_id: "release-storage".into(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap()
        .driver
        .unwrap();
    let context = SnapshotContext {
        session_id: "release-storage".into(),
    };
    let mut state = LoopState::new("release-storage".into());
    let saved = backend.save(context.clone()).unwrap();
    let deleted = backend.save(context.clone()).unwrap();
    backend
        .delete_snapshot(context.clone(), &deleted.id)
        .await
        .unwrap();
    backend.release(context.clone());
    assert!(storage.load(&saved.id).await.unwrap().is_none());
    driver.process_pending(&mut state, &mut 1, false).await;
    assert!(storage.load(&saved.id).await.unwrap().is_some());
    assert!(storage.load(&deleted.id).await.unwrap().is_none());
    assert!(!driver.is_hot().await);
    backend
        .init(
            SnapshotContext {
                session_id: state.session_id.clone(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: saved.id,
            },
        )
        .await
        .unwrap();
    assert!(backend
        .restore(
            context.clone(),
            RestoreRequest {
                mode: RestoreMode::EnsureSafeMode,
                snapshot_id: deleted.id
            }
        )
        .await
        .is_err());
}

#[test]
fn restore_mode_is_required_in_request_json() {
    assert!(
        serde_json::from_value::<RestoreRequest>(serde_json::json!({"snapshot_id":"id"})).is_err()
    );
    for (wire, mode) in [
        ("ensure_safe_mode", RestoreMode::EnsureSafeMode),
        ("promise_mode", RestoreMode::PromiseMode),
    ] {
        let request: RestoreRequest =
            serde_json::from_value(serde_json::json!({"snapshot_id":"id", "mode":wire})).unwrap();
        assert_eq!(request.mode, mode);
    }
}

#[tokio::test]
async fn restore_modes_accept_only_their_id_states_and_preserve_fifo() {
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    let mut turn = 1;
    push(&mut state, "base");
    let base = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    push(&mut state, "future");
    let pending = backend.save(context.clone()).unwrap();
    assert!(matches!(
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    snapshot_id: pending.id.clone(),
                    mode: RestoreMode::EnsureSafeMode
                }
            )
            .await,
        Err(CheckpointError::SnapshotNotReadyOrNotFound(_))
    ));
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: base.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: pending.id.clone(),
                mode: RestoreMode::PromiseMode,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(
        state.messages.read().len(),
        2,
        "promise restores the new snapshot after the intervening restore of base"
    );
    for mode in [RestoreMode::EnsureSafeMode, RestoreMode::PromiseMode] {
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    snapshot_id: pending.id.clone(),
                    mode,
                },
            )
            .await
            .unwrap();
    }
    backend
        .delete_snapshot(context.clone(), &pending.id)
        .await
        .unwrap();
    let deleted_while_pending = backend.save(context.clone()).unwrap();
    backend
        .delete_snapshot(context.clone(), &deleted_while_pending.id)
        .await
        .unwrap();
    for mode in [RestoreMode::EnsureSafeMode, RestoreMode::PromiseMode] {
        for snapshot_id in [
            &pending.id,
            &deleted_while_pending.id,
            &SnapshotId("unknown".into()),
        ] {
            assert!(matches!(
                backend
                    .restore(
                        context.clone(),
                        RestoreRequest {
                            snapshot_id: snapshot_id.clone(),
                            mode
                        }
                    )
                    .await,
                Err(CheckpointError::SnapshotNotReadyOrNotFound(_))
            ));
        }
    }
    // Accepted restores still execute before deletion. Completing a logically
    // deleted pending snapshot must not make its ID valid again.
    driver.process_pending(&mut state, &mut turn, false).await;
    for mode in [RestoreMode::EnsureSafeMode, RestoreMode::PromiseMode] {
        assert!(backend
            .restore(
                context.clone(),
                RestoreRequest {
                    snapshot_id: deleted_while_pending.id.clone(),
                    mode
                }
            )
            .await
            .is_err());
    }
}

#[tokio::test]
async fn session_close_invalidates_retained_handles_and_closes_storage() {
    use super::{backend::QueuedCheckpointBackend, storage::StorageFactory};
    let storage = storage::factory().create(StorageConfig::default()).unwrap();
    let backend = Arc::new(QueuedCheckpointBackend::new(
        storage.clone(),
        CheckpointConfig {
            save_mode: SaveMode::Next,
            ..Default::default()
        },
    ));
    let driver = backend
        .init(
            SnapshotContext {
                session_id: "closing-session".into(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap()
        .driver
        .unwrap();
    let context = SnapshotContext {
        session_id: "closing-session".into(),
    };
    let mut state = LoopState::new("closing-session".into());
    let saved = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut 1, false).await;
    let record = storage.load(&saved.id).await.unwrap().unwrap();
    let _pending = backend.save(context.clone()).unwrap();
    backend.close();
    driver.close().await;
    assert!(matches!(
        backend.save(context.clone()),
        Err(CheckpointError::QueueClosed)
    ));
    assert!(matches!(
        backend
            .init(
                SnapshotContext {
                    session_id: state.session_id.clone()
                },
                serde_json::json!({})
            )
            .await,
        Err(CheckpointError::QueueClosed)
    ));
    backend.release(context.clone());
    assert!(matches!(
        backend
            .init(
                SnapshotContext {
                    session_id: state.session_id.clone()
                },
                serde_json::json!({})
            )
            .await,
        Err(CheckpointError::QueueClosed)
    ));
    assert!(matches!(
        storage.load(&saved.id).await,
        Err(CheckpointError::QueueClosed)
    ));
    assert!(matches!(
        storage.save(record).await,
        Err(CheckpointError::QueueClosed)
    ));
    // References held by a retiring worker cannot revive the old queue/storage.
    driver.process_pending(&mut state, &mut 1, false).await;
    assert!(!driver.is_hot().await);
}

#[cfg(unix)]
async fn checkpoint_rpc(path: &std::path::Path, request: serde_json::Value) -> serde_json::Value {
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixStream,
    };
    let mut stream = UnixStream::connect(path).await.unwrap();
    stream
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn session_switch_creates_fresh_checkpoint_resources() {
    let owner = UnixCheckpointSession::default();
    let config = socket_config();
    let a = uuid::Uuid::new_v4().to_string();
    let b = uuid::Uuid::new_v4().to_string();
    let path_a = owner.prepare(Some(&a), &config).await.unwrap().unwrap();
    assert!(owner.prepare(Some(&a), &config).await.unwrap().is_none());
    assert_eq!(
        checkpoint_rpc(
            &path_a,
            serde_json::json!({"op":"init", "context":{"session_id":a}})
        )
        .await["ok"],
        true
    );
    let snapshot = checkpoint_rpc(
        &path_a,
        serde_json::json!({"op":"save", "context":{"session_id":a}}),
    )
    .await;
    let id = snapshot["result"]["id"].clone();
    let old_driver = owner.driver(&a).await.unwrap();
    let mut state = LoopState::new(a.clone());
    old_driver.process_pending(&mut state, &mut 1, false).await;
    assert_eq!(
        checkpoint_rpc(
            &path_a,
            serde_json::json!({"op":"save", "context":{"session_id":a}})
        )
        .await["ok"],
        true
    ); // pending work may be discarded on session close
    let path_b = owner.prepare(Some(&b), &config).await.unwrap().unwrap();
    assert_eq!(path_a, path_b);
    assert!(path_b.exists());
    assert!(owner.driver(&a).await.is_none());
    assert!(!Arc::ptr_eq(&old_driver, &owner.driver(&b).await.unwrap()));
    old_driver.process_pending(&mut state, &mut 1, false).await;
    assert!(!old_driver.is_hot().await);
    assert_eq!(
        checkpoint_rpc(
            &path_b,
            serde_json::json!({"op":"save", "context":{"session_id":b}})
        )
        .await["ok"],
        false
    );
    assert_eq!(
        checkpoint_rpc(
            &path_b,
            serde_json::json!({"op":"init", "context":{"session_id":b}})
        )
        .await["ok"],
        true
    );
    // A late close of A must not close B.
    owner.close(Some(&a)).await;
    assert!(path_b.exists());
    owner.close(Some(&b)).await;
    assert!(!path_b.exists());
    // Even reopening the old ID allocates fresh storage and requires fresh init.
    let reopened = owner.prepare(Some(&a), &config).await.unwrap().unwrap();
    assert_eq!(
        checkpoint_rpc(
            &reopened,
            serde_json::json!({"op":"save", "context":{"session_id":a}})
        )
        .await["ok"],
        false
    );
    assert_eq!(
        checkpoint_rpc(
            &reopened,
            serde_json::json!({"op":"init", "context":{"session_id":a}})
        )
        .await["ok"],
        true
    );
    assert_eq!(
        checkpoint_rpc(
            &reopened,
            serde_json::json!({"op":"restore", "context":{"session_id":a}, "mode":"promise_mode", "snapshot_id":id})
        )
        .await["ok"],
        false
    );
    owner.close(None).await;
}

#[cfg(unix)]
#[tokio::test]
async fn failed_socket_bind_is_not_retried_on_every_frame_and_recovers_after_mode_change() {
    let config = socket_config();
    let id = uuid::Uuid::new_v4().to_string();
    let occupied = UnixCheckpointEndpoint::bind(id.clone(), config.clone())
        .await
        .unwrap();
    let owner = UnixCheckpointSession::default();
    assert!(owner.prepare(Some(&id), &config).await.is_err());
    assert!(owner.driver(&id).await.is_none());
    assert!(owner.prepare(Some(&id), &config).await.unwrap().is_none());
    occupied.close().await;
    owner.prepare(None, &config).await.unwrap();
    let path = owner.prepare(Some(&id), &config).await.unwrap().unwrap();
    assert!(path.exists());
    owner.close(None).await;
}

#[tokio::test]
async fn list_checkpoints_filters_states_and_preserves_save_order_across_release() {
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    let list = |count| backend.list_checkpoint(context.clone(), AccessMode::LastN, count);
    assert!(list(None).unwrap().is_empty());
    let mut ids = Vec::new();
    for _ in 0..12 {
        ids.push(backend.save(context.clone()).unwrap().id);
    }
    let newest: Vec<_> = ids.iter().rev().cloned().collect();
    assert_eq!(list(None).unwrap(), newest[..10]);
    assert_eq!(list(Some(2)).unwrap(), newest[..2]);
    assert_eq!(list(Some(0)).unwrap(), Vec::<SnapshotId>::new());
    assert_eq!(list(Some(-1)).unwrap(), newest);
    assert_eq!(list(Some(-5)).unwrap(), newest);
    assert_eq!(list(Some(100)).unwrap(), newest);
    driver.process_pending(&mut state, &mut 1, false).await;
    assert_eq!(list(Some(-1)).unwrap(), newest);
    backend
        .delete_snapshot(context.clone(), &ids[10])
        .await
        .unwrap();
    let remaining: Vec<_> = newest.into_iter().filter(|id| id != &ids[10]).collect();
    assert_eq!(list(None).unwrap(), remaining[..10]);
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: ids[0].clone(),
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    assert_eq!(list(Some(-1)).unwrap(), remaining);
    backend.release(context.clone());
    assert!(matches!(list(None), Err(CheckpointError::Released)));
    driver.process_pending(&mut state, &mut 1, false).await;
    backend
        .init(
            SnapshotContext {
                session_id: context.session_id.clone(),
            },
            serde_json::json!({}),
        )
        .await
        .unwrap();
    assert_eq!(list(Some(-1)).unwrap(), remaining);
    assert!(matches!(
        backend.list_checkpoint(
            SnapshotContext {
                session_id: "other-session".into()
            },
            AccessMode::LastN,
            None,
        ),
        Err(CheckpointError::SessionMismatch)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn unix_socket_lists_checkpoint_ids_with_default_count() {
    let session_id = uuid::Uuid::new_v4().to_string();
    let endpoint = UnixCheckpointEndpoint::bind(session_id.clone(), socket_config())
        .await
        .unwrap();
    let request = serde_json::json!({
        "op": "list_checkpoint", "context": {"session_id": session_id},
        "access_mode": "last_n"
    });
    assert_eq!(
        checkpoint_rpc(endpoint.path(), request.clone()).await["ok"],
        false
    );
    checkpoint_rpc(
        endpoint.path(),
        serde_json::json!({"op":"init", "context":{"session_id":session_id}}),
    )
    .await;
    let mut ids = Vec::new();
    for _ in 0..12 {
        let saved = checkpoint_rpc(
            endpoint.path(),
            serde_json::json!({"op":"save", "context":{"session_id":session_id}}),
        )
        .await;
        ids.push(saved["result"]["id"].clone());
    }
    ids.reverse();
    let response = checkpoint_rpc(endpoint.path(), request.clone()).await;
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"], serde_json::json!(&ids[..10]));
    let mut all = request;
    all["count"] = serde_json::json!(-1);
    assert_eq!(
        checkpoint_rpc(endpoint.path(), all).await["result"],
        serde_json::json!(ids)
    );
}

#[tokio::test]
async fn snapshot_context_from_json_scopes_backend_operations() {
    let backend = create(Default::default()).unwrap();
    let context = SnapshotContext::from_json(r#"{"session_id":"context-test"}"#).unwrap();
    assert!(SnapshotContext::from_json("{}")
        .unwrap()
        .session_id
        .is_empty());
    assert!(SnapshotContext::from_json(r#"{"session_id":123}"#).is_err());
    assert!(SnapshotContext::from_json("invalid json").is_err());
    let binding = backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap();
    let snapshot = backend.save(context.clone()).unwrap();
    let other = SnapshotContext::from_json(r#"{"session_id":"other"}"#).unwrap();
    assert!(matches!(
        backend.init(other.clone(), serde_json::json!({})).await,
        Err(CheckpointError::SessionMismatch)
    ));
    assert!(matches!(
        backend.save(other.clone()),
        Err(CheckpointError::SessionMismatch)
    ));
    assert!(matches!(
        backend
            .restore(
                other.clone(),
                RestoreRequest {
                    snapshot_id: snapshot.id.clone(),
                    mode: RestoreMode::PromiseMode,
                }
            )
            .await,
        Err(CheckpointError::SessionMismatch)
    ));
    assert!(matches!(
        backend.delete_snapshot(other.clone(), &snapshot.id).await,
        Err(CheckpointError::SessionMismatch)
    ));
    backend.release(other);
    assert_eq!(
        backend
            .list_checkpoint(context.clone(), AccessMode::LastN, None)
            .unwrap(),
        vec![snapshot.id]
    );
    backend.release(context.clone());
    assert!(matches!(
        backend.save(context),
        Err(CheckpointError::Released)
    ));
    drop(binding);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_socket_init_defaults_to_current_session_and_returns_it() {
    let session_id = uuid::Uuid::new_v4().to_string();
    let endpoint = UnixCheckpointEndpoint::bind(session_id.clone(), socket_config())
        .await
        .unwrap();
    for request in [
        serde_json::json!({"op":"init"}),
        serde_json::json!({"op":"init", "context":{}}),
        serde_json::json!({"op":"init", "context":{"future_field":42}}),
        serde_json::json!({"op":"init", "context":{"session_id":session_id}}),
    ] {
        let response = checkpoint_rpc(endpoint.path(), request).await;
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["session_id"], session_id);
        let context = response["result"].clone();
        assert_eq!(
            checkpoint_rpc(
                endpoint.path(),
                serde_json::json!({
                    "op":"save", "context":context
                })
            )
            .await["ok"],
            true
        );
        assert_eq!(
            checkpoint_rpc(
                endpoint.path(),
                serde_json::json!({
                    "op":"release", "context":context
                })
            )
            .await["ok"],
            true
        );
    }
    let mismatch = checkpoint_rpc(
        endpoint.path(),
        serde_json::json!({
            "op":"init", "context":{"session_id":"wrong-session"}
        }),
    )
    .await;
    assert_eq!(mismatch["ok"], false);
    assert_eq!(
        mismatch["error"],
        CheckpointError::SessionMismatch.to_string()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn socket_configuration_controls_enablement_and_exact_endpoint() {
    let owner = UnixCheckpointSession::default();
    let mut config = socket_config();
    let path = config.unix_socket.as_ref().unwrap().endpoint.clone();
    config.enabled = false;
    assert!(owner
        .prepare(Some("configured"), &config)
        .await
        .unwrap()
        .is_none());
    assert!(!path.exists());
    config.enabled = true;
    assert_eq!(
        owner.prepare(Some("configured"), &config).await.unwrap(),
        Some(path.clone())
    );
    let initialized = checkpoint_rpc(&path, serde_json::json!({"op":"init"})).await;
    assert_eq!(initialized["result"]["session_id"], "configured");
    config.enabled = false;
    owner.prepare(Some("configured"), &config).await.unwrap();
    assert!(!path.exists());
    config.enabled = true;
    config.unix_socket = None;
    assert!(owner.prepare(Some("configured"), &config).await.is_err());
    // Correcting the configuration retries without needing a new session.
    config.unix_socket = Some(UnixSocketConfig {
        endpoint: path.clone(),
    });
    assert_eq!(
        owner.prepare(Some("configured"), &config).await.unwrap(),
        Some(path.clone())
    );
    owner.close(None).await;
    assert!(!path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn socket_bind_reclaims_a_stale_socket() {
    let config = socket_config();
    let path = config.unix_socket.as_ref().unwrap().endpoint.clone();
    // Dropping a raw listener closes it but leaves the filesystem entry,
    // matching the residue left by abrupt process termination.
    drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
    assert!(path.exists());
    let endpoint = UnixCheckpointEndpoint::bind("recovered".into(), config)
        .await
        .unwrap();
    assert_eq!(
        checkpoint_rpc(&path, serde_json::json!({"op":"init"})).await["result"]["session_id"],
        "recovered"
    );
    endpoint.close().await;
    assert!(!path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn socket_bind_preserves_an_active_endpoint() {
    let config = socket_config();
    let endpoint = UnixCheckpointEndpoint::bind("original".into(), config.clone())
        .await
        .unwrap();
    assert!(
        matches!(UnixCheckpointEndpoint::bind("second".into(), config).await,
        Err(CheckpointError::Io(error)) if error.kind() == std::io::ErrorKind::AddrInUse)
    );
    assert_eq!(
        checkpoint_rpc(endpoint.path(), serde_json::json!({"op":"init"})).await["result"]
            ["session_id"],
        "original"
    );
    endpoint.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn socket_bind_preserves_regular_files_and_symlinks() {
    let config = socket_config();
    let path = config.unix_socket.as_ref().unwrap().endpoint.clone();
    std::fs::write(&path, "keep this file").unwrap();
    assert!(UnixCheckpointEndpoint::bind("file".into(), config.clone())
        .await
        .is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep this file");
    std::fs::remove_file(&path).unwrap();
    let target = path.with_extension("target");
    drop(std::os::unix::net::UnixListener::bind(&target).unwrap());
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(UnixCheckpointEndpoint::bind("link".into(), config)
        .await
        .is_err());
    assert!(std::fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(target.exists());
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(target).unwrap();
}

#[tokio::test]
async fn current_boundary_saves_pin_versions_and_restore_publishes_current() {
    use super::{backend::QueuedCheckpointBackend, storage::StorageFactory};
    let storage = storage::factory().create(StorageConfig::default()).unwrap();
    let backend = QueuedCheckpointBackend::new(
        storage.clone(),
        CheckpointConfig {
            save_mode: SaveMode::Current,
            ..Default::default()
        },
    );
    let context = SnapshotContext {
        session_id: "current-boundary".into(),
    };
    let driver = backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap()
        .driver
        .unwrap();
    let mut state = LoopState::new(context.session_id.clone());
    let mut turn = 1;
    assert!(matches!(
        backend.save(context.clone()),
        Err(CheckpointError::NoCurrentCheckpoint)
    ));
    assert!(backend
        .list_checkpoint(context.clone(), AccessMode::LastN, None)
        .unwrap()
        .is_empty());
    push(&mut state, "A");
    driver.process_pending(&mut state, &mut turn, false).await;
    push(&mut state, "B");
    let a1 = backend.save(context.clone()).unwrap();
    let a2 = backend.save(context.clone()).unwrap();
    assert_ne!(a1.id, a2.id);
    assert!(matches!(
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    snapshot_id: a1.id.clone(),
                    mode: RestoreMode::EnsureSafeMode,
                }
            )
            .await,
        Err(CheckpointError::SnapshotNotReadyOrNotFound(_))
    ));
    driver.process_pending(&mut state, &mut turn, false).await;
    let stored_a1 = storage.load(&a1.id).await.unwrap().unwrap();
    let stored_a2 = storage.load(&a2.id).await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&stored_a1.context, &stored_a2.context));
    // Restore queued before save does not change which version save selects.
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: a1.id.clone(),
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    let b = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(state.messages.read().len(), 1);
    assert_eq!(state.messages.read()[0].text_content(), Some("A"));
    let restored_current = backend.save(context.clone()).unwrap();
    backend
        .delete_snapshot(context.clone(), &a1.id)
        .await
        .unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: b.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(state.messages.read().len(), 2);
    assert!(storage.load(&a1.id).await.unwrap().is_none());
    assert!(storage.load(&a2.id).await.unwrap().is_some());
    backend
        .restore(
            context,
            RestoreRequest {
                snapshot_id: restored_current.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut turn, false).await;
    assert_eq!(state.messages.read().len(), 1);
    let encoded = serde_json::to_string(&stored_a2).unwrap();
    let decoded: state::StoredCheckpoint = serde_json::from_str(&encoded).unwrap();
    decoded.context.apply(&mut state, &mut turn);
    assert_eq!(state.messages.read()[0].text_content(), Some("A"));
}

#[tokio::test]
async fn current_boundary_release_discards_current_but_keeps_accepted_saves() {
    use super::{backend::QueuedCheckpointBackend, storage::StorageFactory};
    let storage = storage::factory().create(StorageConfig::default()).unwrap();
    let backend = QueuedCheckpointBackend::new(
        storage.clone(),
        CheckpointConfig {
            save_mode: SaveMode::Current,
            ..Default::default()
        },
    );
    let context = SnapshotContext {
        session_id: "release-current".into(),
    };
    let driver = backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap()
        .driver
        .unwrap();
    let mut state = LoopState::new(context.session_id.clone());
    push(&mut state, "saved boundary");
    driver.process_pending(&mut state, &mut 1, false).await;
    let saved = backend.save(context.clone()).unwrap();
    backend.release(context.clone());
    push(&mut state, "later content");
    driver.flush(&mut state, &mut 2).await;
    let stored = storage.load(&saved.id).await.unwrap().unwrap();
    stored.context.apply(&mut state, &mut 1);
    assert_eq!(state.messages.read().len(), 1);
    backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap();
    assert!(matches!(
        backend.save(context.clone()),
        Err(CheckpointError::NoCurrentCheckpoint)
    ));
    push(&mut state, "new boundary");
    driver.process_pending(&mut state, &mut 2, false).await;
    let pending = backend.save(context.clone()).unwrap();
    backend
        .restore(
            context,
            RestoreRequest {
                snapshot_id: pending.id,
                mode: RestoreMode::PromiseMode,
            },
        )
        .await
        .unwrap();
    state.messages.write().clear();
    driver.flush(&mut state, &mut 3).await;
    assert_eq!(state.messages.read().len(), 2);
}

#[test]
fn current_boundary_unreferenced_versions_are_released() {
    let state = LoopState::new("unreferenced".into());
    let mut capture = factory::create_capture(SaveMode::Current);
    capture.publish(&state, 1);
    let first = capture.select().unwrap().capture(&state, 1);
    let weak = Arc::downgrade(&first);
    drop(first);
    capture.publish(&state, 2);
    assert!(weak.upgrade().is_none());
    let second = capture.select().unwrap().capture(&state, 2);
    let weak = Arc::downgrade(&second);
    drop(second);
    capture.clear();
    assert!(weak.upgrade().is_none());
}

#[test]
fn checkpoint_tail_flag_survives_serialization_and_distinguishes_tool_calls() {
    use agent_types::ContentBlock;
    let state = LoopState::new(uuid::Uuid::new_v4().to_string());
    let tails = [
        (ChatMessage::user("task"), false),
        (
            ChatMessage::tool_result("call", "bash", "output", false, 0),
            false,
        ),
        (
            ChatMessage::new(
                MessageRole::Assistant,
                vec![ContentBlock::ToolUse {
                    call_id: "call".into(),
                    tool_name: "bash".into(),
                    input: serde_json::json!({}),
                }],
                None,
                0,
                None,
            ),
            false,
        ),
        (
            ChatMessage::text(MessageRole::Assistant, "final response", 0),
            true,
        ),
    ];
    for (tail, expected) in tails {
        *state.messages.write() = vec![tail];
        let context = state::ContextState::capture(&state, 1);
        let json = serde_json::to_value(&context).unwrap();
        assert_eq!(json["ends_with_response"], expected);
        let restored: state::ContextState = serde_json::from_value(json).unwrap();
        let mut target = LoopState::new(uuid::Uuid::new_v4().to_string());
        restored.restore(&mut target, &mut 1, true, None);
        assert_eq!(target.messages.read().len(), if expected { 0 } else { 1 });
    }
}

#[tokio::test]
async fn checkpoint_classifies_boundary_once_before_restoring_turn_and_history() {
    for (mut turn, append, keep_input) in [(1, true, true), (1, false, false), (2, true, false)] {
        let (backend, driver, mut state) = setup().await;
        let context = SnapshotContext {
            session_id: state.session_id.clone(),
        };
        let saved = vec![
            ChatMessage::user("A"),
            ChatMessage::text(MessageRole::Assistant, "answer", 0),
        ];
        *state.messages.write() = saved.clone();
        let snapshot = backend.save(context.clone()).unwrap();
        driver.flush(&mut state, &mut 7).await;
        let input = ChatMessage::user("I");
        state.messages.write().push(input.clone());
        // Both restores must use entry state, even after the first rewinds to turn 7.
        for _ in 0..2 {
            backend
                .restore(
                    context.clone(),
                    RestoreRequest {
                        snapshot_id: snapshot.id.clone(),
                        mode: RestoreMode::EnsureSafeMode,
                    },
                )
                .await
                .unwrap();
        }
        driver.process_pending(&mut state, &mut turn, append).await;
        let expected = if keep_input {
            vec![saved[0].clone(), saved[1].clone(), input]
        } else {
            vec![saved[0].clone()]
        };
        assert_eq!(*state.messages.read(), expected);
        assert_eq!(turn, 7);
    }
}

#[tokio::test]
async fn checkpoint_load_failure_leaves_context_unchanged() {
    use super::{
        backend::QueuedCheckpointBackend, state::StoredCheckpoint, storage::CheckpointStorage,
    };
    use async_trait::async_trait;
    struct FailedLoad;
    #[async_trait]
    impl CheckpointStorage for FailedLoad {
        fn close(&self) {}
        async fn save(&self, _: StoredCheckpoint) -> Result<Option<SnapshotId>, CheckpointError> {
            Ok(None)
        }
        async fn load(&self, _: &SnapshotId) -> Result<Option<StoredCheckpoint>, CheckpointError> {
            Err(CheckpointError::Storage("load failed".into()))
        }
        async fn delete(&self, _: &SnapshotId) -> Result<(), CheckpointError> {
            Ok(())
        }
    }
    let context = SnapshotContext {
        session_id: uuid::Uuid::new_v4().to_string(),
    };
    let backend = QueuedCheckpointBackend::new(
        Arc::new(FailedLoad),
        CheckpointConfig {
            save_mode: SaveMode::Next,
            ..Default::default()
        },
    );
    let driver = backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap()
        .driver
        .unwrap();
    let mut state = LoopState::new(context.session_id.clone());
    push(&mut state, "unchanged");
    let original = serde_json::to_value(state::ContextState::capture(&state, 1)).unwrap();
    let snapshot = backend.save(context.clone()).unwrap();
    driver.process_pending(&mut state, &mut 1, true).await;
    backend
        .restore(
            context,
            RestoreRequest {
                snapshot_id: snapshot.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    driver.process_pending(&mut state, &mut 1, true).await;
    assert_eq!(
        serde_json::to_value(state::ContextState::capture(&state, 1)).unwrap(),
        original
    );
}

#[tokio::test]
async fn memory_storage_fifo_reads_overwrites_and_deletes_preserve_save_order() {
    use super::{state::StoredCheckpoint, storage::StorageFactory};
    let storage = storage::factory()
        .create(StorageConfig {
            memory: Some(MemoryStorageConfig { capacity: Some(2) }),
            ..Default::default()
        })
        .unwrap();
    let state = LoopState::new(uuid::Uuid::new_v4().to_string());
    let checkpoint = |id: &str| StoredCheckpoint {
        snapshot: Snapshot {
            id: SnapshotId(id.into()),
            session_id: state.session_id.clone(),
        },
        context: Arc::new(state::ContextState::capture(&state, 1)),
    };
    let a = checkpoint("a");
    let b = checkpoint("b");
    let c = checkpoint("c");
    assert!(storage.save(a.clone()).await.unwrap().is_none());
    assert!(storage.save(b.clone()).await.unwrap().is_none());
    assert!(storage.load(&a.snapshot.id).await.unwrap().is_some());
    assert!(storage.save(a.clone()).await.unwrap().is_none());
    assert_eq!(
        storage.save(c.clone()).await.unwrap(),
        Some(a.snapshot.id.clone())
    );
    assert!(storage.load(&a.snapshot.id).await.unwrap().is_none());
    assert!(storage.load(&b.snapshot.id).await.unwrap().is_some());
    storage.delete(&b.snapshot.id).await.unwrap();
    assert!(storage.save(checkpoint("d")).await.unwrap().is_none());
    assert_eq!(
        storage.save(checkpoint("e")).await.unwrap(),
        Some(c.snapshot.id)
    );
}

#[tokio::test]
async fn memory_capacity_eviction_updates_checkpoint_ids_in_both_save_modes() {
    for mode in [SaveMode::Next, SaveMode::Current] {
        for capacity in [None, Some(0), Some(1), Some(2)] {
            let backend = create(CheckpointConfig {
                save_mode: mode,
                storage: StorageConfig {
                    memory: Some(MemoryStorageConfig { capacity }),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
            let context = SnapshotContext {
                session_id: uuid::Uuid::new_v4().to_string(),
            };
            let driver = backend
                .init(context.clone(), serde_json::json!({}))
                .await
                .unwrap()
                .driver
                .unwrap();
            let mut state = LoopState::new(context.session_id.clone());
            push(&mut state, "saved context");
            driver.flush(&mut state, &mut 1).await;
            let mut ids = Vec::new();
            for _ in 0..21 {
                let snapshot = backend.save(context.clone()).unwrap();
                ids.push(snapshot.id);
                driver.flush(&mut state, &mut 1).await;
            }
            let retained = capacity.unwrap_or(20);
            let expected: Vec<_> = ids.iter().rev().take(retained).cloned().collect();
            assert_eq!(
                backend
                    .list_checkpoint(context.clone(), AccessMode::LastN, Some(-1))
                    .unwrap(),
                expected
            );
            for id in &ids {
                for mode in [RestoreMode::EnsureSafeMode, RestoreMode::PromiseMode] {
                    let result = backend
                        .restore(
                            context.clone(),
                            RestoreRequest {
                                snapshot_id: id.clone(),
                                mode,
                            },
                        )
                        .await;
                    if expected.contains(id) {
                        result.unwrap();
                    } else {
                        assert!(matches!(
                            result,
                            Err(CheckpointError::SnapshotNotReadyOrNotFound(_))
                        ));
                    }
                }
            }
            driver.flush(&mut state, &mut 1).await;
            backend.release(context.clone());
            driver.flush(&mut state, &mut 1).await;
            backend
                .init(context.clone(), serde_json::json!({}))
                .await
                .unwrap();
            assert_eq!(
                backend
                    .list_checkpoint(context, AccessMode::LastN, Some(-1))
                    .unwrap(),
                expected
            );
        }
    }
}

#[tokio::test]
async fn memory_fifo_middle_delete_frees_capacity_without_evicting_oldest() {
    for mode in [SaveMode::Next, SaveMode::Current] {
        let backend = create(CheckpointConfig {
            save_mode: mode,
            storage: StorageConfig {
                memory: Some(MemoryStorageConfig { capacity: Some(3) }),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
        let context = SnapshotContext {
            session_id: uuid::Uuid::new_v4().to_string(),
        };
        let driver = backend
            .init(context.clone(), serde_json::json!({}))
            .await
            .unwrap()
            .driver
            .unwrap();
        let mut state = LoopState::new(context.session_id.clone());
        push(&mut state, "saved context");
        driver.flush(&mut state, &mut 1).await;
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(backend.save(context.clone()).unwrap().id);
            driver.flush(&mut state, &mut 1).await;
        }
        // A, B, C -> delete B -> save D: A must remain, even in the same batch.
        backend
            .delete_snapshot(context.clone(), &ids[1])
            .await
            .unwrap();
        let d = backend.save(context.clone()).unwrap();
        driver.flush(&mut state, &mut 1).await;
        assert_eq!(
            backend
                .list_checkpoint(context.clone(), AccessMode::LastN, Some(-1))
                .unwrap(),
            vec![d.id.clone(), ids[2].clone(), ids[0].clone()]
        );
        backend
            .restore(
                context.clone(),
                RestoreRequest {
                    snapshot_id: ids[0].clone(),
                    mode: RestoreMode::EnsureSafeMode,
                },
            )
            .await
            .unwrap();
        driver.flush(&mut state, &mut 1).await;
        // Only the next insertion exceeds capacity and evicts A.
        let e = backend.save(context.clone()).unwrap();
        driver.flush(&mut state, &mut 1).await;
        assert_eq!(
            backend
                .list_checkpoint(context, AccessMode::LastN, Some(-1))
                .unwrap(),
            vec![e.id, d.id, ids[2].clone()]
        );
    }
}

#[tokio::test]
async fn init_save_mode_overrides_startup_and_defaults() {
    use super::{backend::QueuedCheckpointBackend, storage::StorageFactory};
    for startup in [SaveMode::Next, SaveMode::Current] {
        for override_mode in [None, Some(SaveMode::Next), Some(SaveMode::Current)] {
            let storage = storage::factory().create(StorageConfig::default()).unwrap();
            let backend = QueuedCheckpointBackend::new(
                storage.clone(),
                CheckpointConfig {
                    save_mode: startup,
                    ..Default::default()
                },
            );
            let context = SnapshotContext {
                session_id: "init-options".into(),
            };
            let driver = backend
                .init(
                    context.clone(),
                    serde_json::json!({"save_mode": override_mode}),
                )
                .await
                .unwrap()
                .driver
                .unwrap();
            let mut state = LoopState::new(context.session_id.clone());
            push(&mut state, "published");
            driver.process_pending(&mut state, &mut 1, false).await;
            push(&mut state, "later");
            let snapshot = backend.save(context).unwrap();
            driver.process_pending(&mut state, &mut 2, false).await;
            let stored = storage.load(&snapshot.id).await.unwrap().unwrap();
            let expected = if override_mode.unwrap_or(startup) == SaveMode::Current {
                1
            } else {
                2
            };
            stored.context.restore(&mut state, &mut 2, false, None);
            assert_eq!(state.messages.read().len(), expected);
        }
    }
    assert_eq!(CheckpointConfig::default().save_mode, SaveMode::Next);
}

#[tokio::test]
async fn reinit_resolves_options_without_changing_accepted_saves() {
    use super::{backend::QueuedCheckpointBackend, storage::StorageFactory};
    let storage = storage::factory().create(StorageConfig::default()).unwrap();
    let backend = QueuedCheckpointBackend::new(
        storage.clone(),
        CheckpointConfig {
            save_mode: SaveMode::Next,
            ..Default::default()
        },
    );
    let context = SnapshotContext {
        session_id: "reinit-options".into(),
    };
    let driver = backend
        .init(
            context.clone(),
            serde_json::json!({"save_mode": Some(SaveMode::Current)}),
        )
        .await
        .unwrap()
        .driver
        .unwrap();
    let mut state = LoopState::new(context.session_id.clone());
    push(&mut state, "published");
    driver.process_pending(&mut state, &mut 1, false).await;
    let current = backend.save(context.clone()).unwrap();
    backend.release(context.clone());
    // Omission restores the startup mode, even before Release is processed.
    backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap();
    let next = backend.save(context.clone()).unwrap();
    backend.release(context.clone());
    backend
        .init(
            context.clone(),
            serde_json::json!({"save_mode": Some(SaveMode::Current)}),
        )
        .await
        .unwrap();
    assert!(matches!(
        backend.save(context.clone()),
        Err(CheckpointError::NoCurrentCheckpoint)
    ));
    push(&mut state, "later");
    driver.process_pending(&mut state, &mut 2, false).await;
    storage
        .load(&current.id)
        .await
        .unwrap()
        .unwrap()
        .context
        .restore(&mut state, &mut 2, false, None);
    assert_eq!(state.messages.read().len(), 1);
    storage
        .load(&next.id)
        .await
        .unwrap()
        .unwrap()
        .context
        .restore(&mut state, &mut 2, false, None);
    assert_eq!(state.messages.read().len(), 2);
    assert!(backend.save(context).is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn socket_init_accepts_save_mode_override_and_rejects_invalid_options() {
    use serde_json::json;
    let config = socket_config();
    let endpoint = UnixCheckpointEndpoint::bind("init-options".into(), config)
        .await
        .unwrap();
    for request in [
        json!({"op":"init", "config":{"save_mode":"invalid"}}),
        json!({"op":"init", "unknown":true}),
        json!({"op":"init", "config":{"unknown":true}}),
        json!({"op":"init", "save_mode":"current"}),
        json!({"op":"init", "config":[]}),
        json!({"op":"init", "config":null}),
    ] {
        assert_eq!(checkpoint_rpc(endpoint.path(), request).await["ok"], false);
    }
    assert_eq!(
        checkpoint_rpc(
            endpoint.path(),
            json!({"op":"init", "config":{"save_mode":"current"}})
        )
        .await["ok"],
        true
    );
    let result = checkpoint_rpc(
        endpoint.path(),
        json!({"op":"save", "context":{"session_id":"init-options"}}),
    )
    .await;
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"], "no current checkpoint is available yet");
}

#[tokio::test]
async fn invalid_init_json_does_not_bind_backend() {
    let backend = create(Default::default()).unwrap();
    let context = SnapshotContext {
        session_id: "invalid-config".into(),
    };
    for config in [
        serde_json::json!({"save_mode": "invalid"}),
        serde_json::json!({"unknown": true}),
        serde_json::json!(null),
        serde_json::json!(42),
    ] {
        assert!(matches!(
            backend.init(context.clone(), config).await,
            Err(CheckpointError::InvalidConfig(_))
        ));
        assert!(matches!(
            backend.save(context.clone()),
            Err(CheckpointError::NotInitialized)
        ));
    }
    let _binding = backend
        .init(context.clone(), serde_json::json!({}))
        .await
        .unwrap();
    assert!(backend.save(context).is_ok());
}

#[tokio::test]
async fn snapshot_id_suffix_cycles_and_survives_reinit() {
    let (backend, _driver, state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    let mut ids = std::collections::HashSet::new();
    for index in 0..28 {
        if index == 25 {
            backend.release(context.clone());
            backend
                .init(context.clone(), serde_json::json!({}))
                .await
                .unwrap();
        }
        let snapshot = backend.save(context.clone()).unwrap();
        let (prefix, suffix) = snapshot.id.0.rsplit_once('_').unwrap();
        assert_eq!(suffix, char::from(b'a' + index % 26).to_string());
        let uuid = prefix.strip_prefix("ctxcp_").unwrap();
        assert_eq!(uuid.len(), 32);
        assert!(uuid::Uuid::parse_str(uuid).is_ok());
        assert!(ids.insert(snapshot.id));
    }
    let (other, _other_driver, other_state) = setup().await;
    let snapshot = other
        .save(SnapshotContext {
            session_id: other_state.session_id,
        })
        .unwrap();
    assert!(snapshot.id.0.ends_with("_a"));
}

#[tokio::test]
async fn restore_notification_contains_the_saved_todos() {
    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<CheckpointTodoItem>>);
    impl CheckpointUpdateSink for Sink {
        fn on_restore(
            &self,
            _: &str,
            _: &[agent_types::ChatMessage],
            todos: &[CheckpointTodoItem],
        ) {
            *self.0.lock().unwrap() = todos.to_vec();
        }
    }
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    let todos: Vec<CheckpointTodoItem> = serde_json::from_value(serde_json::json!([
        {"content":"saved todo", "status":"pending"}
    ]))
    .unwrap();
    tool::restore_checkpoint_todos(&state.session_id, todos.clone());
    let snapshot = backend.save(context.clone()).unwrap();
    driver.flush(&mut state, &mut 1).await;
    tool::restore_checkpoint_todos(&state.session_id, Vec::new());
    let sink = Arc::new(Sink::default());
    driver.set_update_sink(sink.clone());
    backend
        .restore(
            context,
            RestoreRequest {
                snapshot_id: snapshot.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    driver.flush(&mut state, &mut 1).await;
    assert_eq!(*sink.0.lock().unwrap(), todos);
    assert_eq!(tool::checkpoint_todos(&state.session_id), todos);
    tool::restore_checkpoint_todos(&state.session_id, Vec::new());
}

#[tokio::test]
async fn checkpoint_defers_save_and_restore_until_all_tool_results_arrive() {
    use agent_types::ContentBlock;
    let (backend, driver, mut state) = setup().await;
    let context = SnapshotContext {
        session_id: state.session_id.clone(),
    };
    push(&mut state, "base");
    let base = backend.save(context.clone()).unwrap();
    driver.flush(&mut state, &mut 1).await;
    state.messages.write().push(ChatMessage::new(
        MessageRole::Assistant,
        ["first", "second"]
            .into_iter()
            .map(|id| ContentBlock::ToolUse {
                call_id: id.into(),
                tool_name: "bash".into(),
                input: serde_json::json!({}),
            })
            .collect(),
        None,
        0,
        None,
    ));
    let saved = backend.save(context.clone()).unwrap();
    backend
        .restore(
            context.clone(),
            RestoreRequest {
                snapshot_id: base.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    for id in ["first", "second"] {
        let before = state.messages.read().clone();
        assert!(!driver.flush(&mut state, &mut 2).await);
        driver.process_pending(&mut state, &mut 2, false).await;
        assert_eq!(*state.messages.read(), before);
        assert!(matches!(
            backend
                .restore(
                    context.clone(),
                    RestoreRequest {
                        snapshot_id: saved.id.clone(),
                        mode: RestoreMode::EnsureSafeMode,
                    }
                )
                .await,
            Err(CheckpointError::SnapshotNotReadyOrNotFound(_))
        ));
        state
            .messages
            .write()
            .push(ChatMessage::tool_result(id, "bash", "done", false, 0));
    }
    let completed = state.messages.read().clone();
    assert!(driver.flush(&mut state, &mut 2).await);
    assert_eq!(state.messages.read().len(), 1);
    backend
        .restore(
            context,
            RestoreRequest {
                snapshot_id: saved.id,
                mode: RestoreMode::EnsureSafeMode,
            },
        )
        .await
        .unwrap();
    assert!(driver.flush(&mut state, &mut 2).await);
    assert_eq!(*state.messages.read(), completed);
}
