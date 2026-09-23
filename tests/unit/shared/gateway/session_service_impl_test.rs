use super::*;
use crate::backend::GatewayBackendConfig;
use crate::gateway::{
    AppBootstrap, GatewayEntryContext, InMemorySessionStore, SessionInput, SessionInputKind,
    SessionRuntimeBindings, SessionRuntimeDescriptor, TurnOutcome, ORPHAN_SESSION_THRESHOLD_MS,
    STALE_LEASE_THRESHOLD_MS,
};
use agent_contracts::backend::BackendLifecycleState;
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::HookerRegistryConfig;
use agent_types::{
    AssistantMessage, ChatMessage, ContentBlock, LlmError, LlmRequest, LlmResponse, StopReason,
    StreamChunk, Usage,
};
use hook::framework::HookerRegistryBuilderImpl;
use hook::HookerRegistryBuilder;
use llm_client::LlmProviderWrapper;
use serde_json::{json, Value};
use std::sync::Mutex as StdMutex;
use tempfile::TempDir;
use xiaoo_api::runtime::LoopStateSnapshot;

use agent_contracts::{Hooker, RuntimeView};
use agent_types::hook::{HookInvokeError, HookInvokeOutput};
use agent_types::session::{SessionHookError, SessionHookResult};
use hook::framework::HookerRegistryImpl;
use std::any::Any;
use std::collections::HashSet;

#[test]
fn runtime_exec_shell_prefers_explicit_shell() {
    assert_eq!(
        resolve_runtime_exec_shell(Some("/bin/zsh".to_string()), Some("/bin/bash")),
        "/bin/zsh"
    );
}

#[test]
fn runtime_exec_shell_uses_backend_default() {
    assert_eq!(
        resolve_runtime_exec_shell(None, Some("/bin/bash")),
        "/bin/bash"
    );
}

#[test]
fn runtime_exec_shell_falls_back_to_posix_shell() {
    assert_eq!(resolve_runtime_exec_shell(None, None), "/bin/sh");
}

#[test]
fn stamp_and_cap_stamps_send_prompt_with_next_depth() {
    // A plugin omits `chain_depth` (parses to 0); the daemon overwrites
    // it with `emitting_turn_depth + 1` regardless of the plugin value.
    let actions = vec![HookAction::SendPrompt {
        session_id: "s1".into(),
        text: "hi".into(),
        chain_depth: 0,
    }];
    let stamped = stamp_and_cap_send_prompt_actions(actions, 5, 128);
    assert_eq!(stamped.len(), 1);
    match &stamped[0] {
        HookAction::SendPrompt { chain_depth, .. } => assert_eq!(*chain_depth, 6),
        _ => panic!("expected SendPrompt"),
    }
}

#[test]
fn stamp_and_cap_overwrites_plugin_supplied_depth() {
    // A plugin cannot forge a low depth to bypass the cap: the daemon
    // ignores any plugin-supplied value and stamps the real next depth.
    // Here `next_depth = 129 >= 128` → dropped regardless of the plugin
    // value (emitting depth 128 is itself only reachable if the cap were
    // higher; the point of this test is the override + drop, not the
    // reachability of the emitting depth).
    let actions = vec![HookAction::SendPrompt {
        session_id: "s1".into(),
        text: "hi".into(),
        chain_depth: 0, // plugin tries to look like a depth-0 turn
    }];
    let stamped = stamp_and_cap_send_prompt_actions(actions, 128, 128);
    // next = 129 >= 128 cap → dropped, not forwarded.
    assert!(stamped.is_empty());
}

#[test]
fn stamp_and_cap_drops_send_prompt_above_cap() {
    let actions = vec![HookAction::SendPrompt {
        session_id: "s1".into(),
        text: "hi".into(),
        chain_depth: 0,
    }];
    // emitting depth = cap (3) → next = 4 >= 3 → dropped (way above cap).
    assert!(stamp_and_cap_send_prompt_actions(actions, 3, 3).is_empty());
}

#[test]
fn stamp_and_cap_drops_send_prompt_at_boundary() {
    // Cap is an EXCLUSIVE upper bound: a `send_prompt` whose next-turn
    // depth would equal `max_depth` is dropped. `max_depth = N` permits
    // turns at depths `0..=N-1` (N turns total); the `N`-th-turn-after-
    // user (depth `N`) must not start.
    //
    // emitting depth = cap - 1 = 2 → next = 3 == cap → dropped.
    let actions = vec![HookAction::SendPrompt {
        session_id: "s1".into(),
        text: "hi".into(),
        chain_depth: 0,
    }];
    let stamped = stamp_and_cap_send_prompt_actions(actions, 2, 3);
    assert!(
        stamped.is_empty(),
        "next_depth == max_depth must be dropped (exclusive cap)"
    );
}

#[test]
fn stamp_and_cap_keeps_send_prompt_just_below_boundary() {
    // emitting depth = cap - 2 = 1 → next = 2 = cap - 1 < cap → the
    // last allowed turn (depth `max - 1`) is permitted to run. With
    // max=3 this is the 3rd turn in the chain (depths 0, 1, 2).
    let actions = vec![HookAction::SendPrompt {
        session_id: "s1".into(),
        text: "hi".into(),
        chain_depth: 0,
    }];
    let stamped = stamp_and_cap_send_prompt_actions(actions, 1, 3);
    assert_eq!(stamped.len(), 1);
    match &stamped[0] {
        HookAction::SendPrompt { chain_depth, .. } => assert_eq!(*chain_depth, 2),
        _ => panic!("expected SendPrompt"),
    }
}

#[test]
fn stamp_and_cap_passes_other_action_kinds_through_unchanged() {
    let actions = vec![
        HookAction::CreateSession {
            session_id: "a".into(),
        },
        HookAction::SwitchSession {
            session_id: "a".into(),
        },
    ];
    let stamped = stamp_and_cap_send_prompt_actions(actions, 999, 1);
    assert_eq!(stamped.len(), 2);
    assert!(matches!(stamped[0], HookAction::CreateSession { .. }));
    assert!(matches!(stamped[1], HookAction::SwitchSession { .. }));
}

#[test]
fn stamp_and_cap_mixed_batch_keeps_non_send_and_caps_send() {
    let actions = vec![
        HookAction::CreateSession {
            session_id: "a".into(),
        },
        HookAction::SendPrompt {
            session_id: "a".into(),
            text: "x".into(),
            chain_depth: 0,
        },
        HookAction::SwitchSession {
            session_id: "a".into(),
        },
        HookAction::SendPrompt {
            session_id: "b".into(),
            text: "y".into(),
            chain_depth: 0,
        },
    ];
    // emitting depth 3, cap 3: first send_prompt → next 4 >= 3 dropped;
    // second send_prompt also dropped; create/switch pass through.
    let stamped = stamp_and_cap_send_prompt_actions(actions, 3, 3);
    assert_eq!(stamped.len(), 2);
    assert!(matches!(stamped[0], HookAction::CreateSession { .. }));
    assert!(matches!(stamped[1], HookAction::SwitchSession { .. }));
}

struct StubLlmProvider {
    capabilities: ProviderCapabilities,
}

#[async_trait]
impl LlmProvider for StubLlmProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        Err(LlmError::RequestFailed {
            message: "stub provider is not expected to complete in session tests".to_string(),
        })
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        Err(LlmError::RequestFailed {
            message: "stub provider is not expected to stream in session tests".to_string(),
        })
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

struct StubRuntimeResolver {
    workspace_root: std::path::PathBuf,
    backend_options: Value,
    llm_provider: Arc<LlmProviderWrapper>,
    /// Mirrors the local TUI wiring: `SessionGateway::spawn_turn` injects
    /// the TUI's Esc-cancel token into the resolver's static bindings.
    cancel_token: Option<tokio_util::sync::CancellationToken>,
}

#[async_trait]
impl SessionRuntimeResolver for StubRuntimeResolver {
    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        _existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        if request.workspace.as_deref() == Some(std::path::Path::new("/conflict")) {
            return Err(SessionRuntimeResolveError::BootstrapConflict {
                message: "test conflict".to_string(),
            });
        }
        Ok(ResolvedSessionRuntime {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId("test-agent".to_string()),
                model: "stub-model".to_string(),
                llm: None,
                system_prompt: "test system".to_string(),
                feature_flags: FeatureFlags::default(),
                token_budget: TokenBudgetConfig {
                    total_budget: 4096,
                    reserved_for_output: 1024,
                    reserved_for_system: 256,
                    hard_limit_ratio: 0.9,
                },
                workspace_root: self.workspace_root.clone(),
                max_turns: None,
                subagent_roles: BTreeMap::new(),
            },
            entry_kind: request.entry.kind.clone(),
            llm_provider: Arc::clone(&self.llm_provider),
            tool_registry: None,
            skill_registry: None,
            bindings: SessionRuntimeBindings {
                cancel_token: self.cancel_token.clone(),
                ..SessionRuntimeBindings::default()
            },
            compression_pipeline: None,
            trace: json!({}),
            hooker: Default::default(),
            operation_backend: Some(GatewayBackendConfig::new(
                "local",
                self.backend_options.clone(),
            )),
            backend_workspace_root: self.workspace_root.clone(),
            e2b_bootstrap: None,
            bootstrap_binding: None,
            e2b_finalized: false,
        })
    }
}

fn stub_llm_provider() -> Arc<LlmProviderWrapper> {
    Arc::new(LlmProviderWrapper::new(
        Arc::new(StubLlmProvider {
            capabilities: ProviderCapabilities {
                supports_streaming: false,
                supports_tool_calls: false,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "stub-model".to_string(),
            },
        }),
        None,
        None,
    ))
}

/// Resolver decorator that parks in `resolve` before delegating, letting
/// tests submit a cancel while a turn is still inside `run_turn_inner`'s
/// resolution phase (submitted, but not yet queued in the session
/// actor).
struct DelayedResolver {
    inner: StubRuntimeResolver,
    delay: std::time::Duration,
}

#[async_trait]
impl SessionRuntimeResolver for DelayedResolver {
    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        tokio::time::sleep(self.delay).await;
        self.inner.resolve(request, existing).await
    }
}

struct ReplyingLlmProvider {
    capabilities: ProviderCapabilities,
    seen_requests: Arc<StdMutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl LlmProvider for ReplyingLlmProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        self.seen_requests
            .lock()
            .expect("seen requests")
            .push(request.clone());
        Ok(reply_response())
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        self.seen_requests
            .lock()
            .expect("seen requests")
            .push(request.clone());
        on_chunk(StreamChunk {
            delta_text: Some("reply".to_string()),
            delta_reasoning: None,
            delta_tool_call: None,
        });
        Ok(reply_response())
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

fn reply_response() -> LlmResponse {
    LlmResponse {
        message: AssistantMessage {
            text: Some("reply".to_string()),
            reasoning_content: None,
            tool_calls: Vec::new(),
            usage: Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cached_tokens: 0,
            },
            stop_reason: StopReason::EndTurn,
        },
        kv_cache_chunk_hashes: Vec::new(),
    }
}

fn replying_llm_provider(seen_requests: Arc<StdMutex<Vec<LlmRequest>>>) -> Arc<LlmProviderWrapper> {
    Arc::new(LlmProviderWrapper::new(
        Arc::new(ReplyingLlmProvider {
            capabilities: ProviderCapabilities {
                supports_streaming: false,
                supports_tool_calls: false,
                supports_json_mode: false,
                max_context_window: 4096,
                model_name: "stub-model".to_string(),
            },
            seen_requests,
        }),
        None,
        None,
    ))
}

#[derive(Default)]
struct FailingRecallAutomation {
    seen_contexts: StdMutex<Vec<TurnMemoryContext>>,
    enqueued: StdMutex<Vec<CompletedTurnIngest>>,
    context_messages: usize,
}

#[async_trait]
impl TurnMemoryAutomation for FailingRecallAutomation {
    async fn recall(
        &self,
        context: &TurnMemoryContext,
    ) -> Result<
        Vec<crate::gateway::memory_automation::RecallMemory>,
        crate::gateway::memory_automation::MemoryAutomationError,
    > {
        self.seen_contexts
            .lock()
            .expect("seen contexts")
            .push(context.clone());
        Err(
            crate::gateway::memory_automation::MemoryAutomationError::Config(
                "forced recall failure".to_string(),
            ),
        )
    }

    async fn enqueue_ingest(
        &self,
        ingest: CompletedTurnIngest,
    ) -> Result<(), crate::gateway::memory_automation::MemoryAutomationError> {
        self.enqueued.lock().expect("enqueued").push(ingest);
        Ok(())
    }

    fn recall_token_budget(&self) -> usize {
        80
    }

    fn context_messages(&self) -> usize {
        self.context_messages
    }
}

fn text_blocks(messages: &[ChatMessage], role: agent_types::MessageRole) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message.role == role)
        .flat_map(|message| &message.blocks)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn test_open_request(session_id: &str) -> SessionOpenRequest {
    SessionOpenRequest {
        session_id: session_id.to_string(),
        conversation_id: format!("{session_id}-conversation"),
        sender_id: "user-1".to_string(),
        entry: GatewayEntryContext::tui(None),
        channel: None,
        channel_instance_id: None,
        llm: None,
        workspace: None,
        skills: None,
        client_id: None,
        client_pid: None,
        client_hostname: None,
    }
}

async fn save_session_without_backend(
    store: &Arc<InMemorySessionStore>,
    resolver: &Arc<StubRuntimeResolver>,
    session_id: &str,
    status: SessionLifecycleStatus,
) -> SessionRecord {
    let request = test_open_request(session_id);
    let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
    let resolved = resolver
        .resolve(&runtime_input, None)
        .await
        .expect("resolve runtime");
    let mut session = CoreBackedSessionService::build_session_for_open(&request, &resolved);
    session.status = status;
    session.backend_instance = None;
    store.save(session.clone()).await;
    session
}

#[tokio::test]
async fn memory_automation_failed_recall_keeps_user_text_and_turn_execution_unchanged() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let seen_requests = Arc::new(StdMutex::new(Vec::new()));
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: replying_llm_provider(Arc::clone(&seen_requests)),
        cancel_token: None,
    });
    let automation = Arc::new(FailingRecallAutomation {
        context_messages: 2,
        ..FailingRecallAutomation::default()
    });
    let dependencies =
        AppBootstrap::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
            store,
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
            Some(automation.clone() as Arc<dyn TurnMemoryAutomation>),
            None,
        )
        .expect("dependencies");

    let result = dependencies
        .session_service
        .run_turn(test_open_request("memory-fail-open").into_turn_request("hello".to_string()))
        .await
        .expect("turn should continue after memory recall failure");

    assert_eq!(result.visible_reply, "reply");
    let requests = seen_requests.lock().expect("seen requests");
    assert_eq!(
        text_blocks(&requests[0].messages, agent_types::MessageRole::User),
        vec!["hello".to_string()]
    );
    assert!(
        !requests[0]
            .messages
            .iter()
            .flat_map(|message| &message.blocks)
            .any(|block| matches!(block, ContentBlock::Text { text } if text.contains("<untrusted_long_term_memory>"))),
        "failed recall must not add memory context"
    );
    drop(requests);

    let contexts = automation.seen_contexts.lock().expect("seen contexts");
    assert_eq!(contexts[0].query, "hello");
    drop(contexts);
    let enqueued = automation.enqueued.lock().expect("enqueued");
    assert_eq!(enqueued[0].user_text, "hello");
    assert_eq!(enqueued[0].assistant_text, "reply");
    assert!(enqueued[0].recent_messages.is_empty());
    drop(enqueued);

    dependencies
        .session_service
        .run_turn(test_open_request("memory-fail-open").into_turn_request("next".to_string()))
        .await
        .expect("second turn should continue after memory recall failure");
    let enqueued = automation.enqueued.lock().expect("enqueued");
    assert_eq!(enqueued.len(), 2);
    assert_eq!(enqueued[1].recent_messages, vec!["hello", "reply"]);
}

#[tokio::test]
async fn open_session_persists_active_backend_instance() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let record = dependencies
        .session_control_plane
        .open_session(SessionOpenRequest {
            session_id: "s1".to_string(),
            conversation_id: "c1".to_string(),
            sender_id: "u1".to_string(),
            entry: GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
            llm: None,
            workspace: None,
            skills: None,

            client_id: None,
            client_pid: None,
            client_hostname: None,
        })
        .await
        .expect("open session");
    let instance = record.backend_instance.expect("backend instance");
    assert_eq!(instance.state, BackendLifecycleState::Active);
    assert_eq!(instance.session_id, "s1");
    assert!(instance.backend_id.0.starts_with("bkd_"));
    assert_ne!(instance.backend_id.0, "s1");

    let saved = store.load("s1").await.expect("saved session");
    let saved_instance = saved.backend_instance.expect("saved backend instance");
    assert_eq!(saved_instance.state, BackendLifecycleState::Active);
    assert_eq!(saved_instance.backend_id, instance.backend_id);
}

#[tokio::test]
async fn existing_handle_does_not_bypass_bootstrap_binding_validation() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .open_session(test_open_request("binding-fast-path"))
        .await
        .expect("initial open");
    let mut conflicting = test_open_request("binding-fast-path");
    conflicting.workspace = Some(std::path::PathBuf::from("/conflict"));
    let error = dependencies
        .session_control_plane
        .open_session(conflicting)
        .await
        .expect_err("binding conflict should be checked before handle reuse");
    assert!(matches!(error, SessionServiceError::RuntimeConflict { .. }));
}

#[tokio::test]
async fn open_session_preserves_imported_loop_state() {
    // Regression: `/load` imports a SessionRecord (with loop_state
    // containing the LLM's prior message history) into the in-memory
    // store, then the next turn calls `ensure_session_open` ->
    // `open_session`. Before the fix, `open_session` built a fresh
    // SessionRecord via `build_session_for_open` (which sets
    // `loop_state: None`) and overwrote the imported record, so the
    // LLM lost all context even though the TUI still echoed the old
    // chat messages.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver.clone(),
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    // Simulate `/load`: seed the store with an Idle record carrying a
    // non-empty loop_state (two ChatMessages).
    let imported_messages = vec![
        agent_types::ChatMessage {
            role: agent_types::MessageRole::User,
            blocks: vec![agent_types::ContentBlock::Text {
                text: "earlier user prompt".to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        },
        agent_types::ChatMessage {
            role: agent_types::MessageRole::Assistant,
            blocks: vec![agent_types::ContentBlock::Text {
                text: "earlier assistant reply".to_string(),
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
            estimated_tokens: None,
        },
    ];
    let mut seeded =
        save_session_without_backend(&store, &resolver, "s-import", SessionLifecycleStatus::Idle)
            .await;
    seeded.loop_state = Some(LoopStateSnapshot {
        session_id: "s-import".to_string(),
        messages: imported_messages.clone(),
        turn_count: 1,
        token_usage: Default::default(),
        compression_meta: Default::default(),
        kv_cache_map: Default::default(),
    });
    store.save(seeded.clone()).await;

    // Now the user sends a new prompt -> `open_session` runs.
    let opened = dependencies
        .session_control_plane
        .open_session(SessionOpenRequest {
            session_id: "s-import".to_string(),
            conversation_id: "c-import".to_string(),
            sender_id: "u1".to_string(),
            entry: GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
            llm: None,
            workspace: None,
            skills: None,

            client_id: None,
            client_pid: None,
            client_hostname: None,
        })
        .await
        .expect("open imported session");

    let retained = opened
        .loop_state
        .as_ref()
        .expect("loop_state must be preserved across open_session");
    assert_eq!(retained.messages, imported_messages);
    assert_eq!(retained.turn_count, 1);

    // The build path (not the `handle_for_session` shortcut) must have
    // run: a backend instance is leased and persisted, proving the
    // state-preservation block above was actually executed.
    let leased_instance = opened
        .backend_instance
        .as_ref()
        .expect("open_session must lease a backend for imported records");
    assert_eq!(leased_instance.session_id, "s-import");
    assert_eq!(leased_instance.state, BackendLifecycleState::Active);

    // And the store must keep the preserved state, not a wiped record.
    let stored = store
        .load("s-import")
        .await
        .expect("store must retain record");
    assert_eq!(
        stored
            .loop_state
            .as_ref()
            .expect("stored loop_state must be preserved")
            .messages,
        imported_messages
    );
    let stored_instance = stored
        .backend_instance
        .as_ref()
        .expect("store must retain leased backend instance");
    assert_eq!(stored_instance.backend_id, leased_instance.backend_id);
}

#[tokio::test]
async fn submit_cancel_active_turn_routes_through_session_handle() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .open_session(SessionOpenRequest {
            session_id: "s-cancel".to_string(),
            conversation_id: "c1".to_string(),
            sender_id: "u1".to_string(),
            entry: GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
            llm: None,
            workspace: None,
            skills: None,

            client_id: None,
            client_pid: None,
            client_hostname: None,
        })
        .await
        .expect("open session");

    let receipt = dependencies
        .session_control_plane
        .submit_input("s-cancel", SessionInput::CancelActiveTurn)
        .await
        .expect("cancel should be accepted");

    assert_eq!(receipt.session_id, "s-cancel");
    assert_eq!(receipt.accepted_kind, SessionInputKind::CancelActiveTurn);
}

/// The local TUI path wires its Esc-cancel token into the resolver's
/// static `SessionRuntimeBindings::cancel_token` and calls plain
/// `run_turn` (no cancellation_token argument). `run_turn_inner` must
/// preserve that resolver-provided token — a pre-cancelled token must
/// drive the agent loop out through `AgentOutcome::Cancelled` (observed
/// here as `TurnOutcome::Cancelled`), not run the turn to completion.
#[tokio::test]
async fn resolver_bound_cancel_token_reaches_agent_loop() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: Some(token),
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let result = dependencies
        .session_service
        .run_turn(test_open_request("s-resolver-cancel").into_turn_request("hi".to_string()))
        .await
        .expect("turn should complete");

    assert_eq!(result.outcome, TurnOutcome::Cancelled);
}

/// An explicit caller-provided cancellation token (e.g. the MCP server
/// path via `run_turn_with_interaction`) must win over a resolver-bound
/// token: the pre-cancelled caller token drives the loop to
/// `TurnOutcome::Cancelled` even though the resolver's own token stays
/// un-cancelled.
#[tokio::test]
async fn caller_cancel_token_overrides_resolver_bound_token() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    // Resolver-bound token never fires: proves the caller's token won.
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: Some(tokio_util::sync::CancellationToken::new()),
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let caller_token = tokio_util::sync::CancellationToken::new();
    caller_token.cancel();
    let result = dependencies
        .session_service
        .run_turn_with_interaction(
            test_open_request("s-caller-cancel").into_turn_request("hi".to_string()),
            None,
            None,
            None,
            Some(caller_token),
            None,
        )
        .await
        .expect("turn should complete");

    assert_eq!(result.outcome, TurnOutcome::Cancelled);
}

/// A `CancelActiveTurn` that arrives while the session is truly idle
/// (no active turn, none queued — e.g. Esc landed just after the turn
/// had already finished) must NOT poison the user's NEXT message: the
/// actor only applies an idle cancel to turns submitted before it, so
/// the fresh turn runs to completion instead of being swallowed with a
/// spurious `cancelled` outcome.
#[tokio::test]
async fn cancel_while_session_idle_does_not_cancel_later_turn() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let seen_requests = Arc::new(StdMutex::new(Vec::new()));
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: replying_llm_provider(Arc::clone(&seen_requests)),
        cancel_token: None,
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .open_session(test_open_request("s-idle-cancel"))
        .await
        .expect("open session");

    // Session is idle (no active turn, nothing queued): the cancel is
    // remembered with its arrival time but applies to nothing.
    let receipt = dependencies
        .session_control_plane
        .submit_input("s-idle-cancel", SessionInput::CancelActiveTurn)
        .await
        .expect("cancel should be accepted");
    assert_eq!(receipt.accepted_kind, SessionInputKind::CancelActiveTurn);

    // A turn submitted afterwards is a fresh user action: it must run
    // normally, not inherit the stale cancel.
    let result = dependencies
        .session_service
        .run_turn(test_open_request("s-idle-cancel").into_turn_request("hi".to_string()))
        .await
        .expect("turn should complete");
    assert_eq!(result.outcome, TurnOutcome::Complete);
}

/// The startup race the idle-cancel latch exists for: Esc's cancel RPC
/// lands while the turn is still in flight inside `run_turn_inner`
/// (resolution has not finished, so the actor has no active turn and
/// nothing queued yet). The turn — submitted BEFORE the cancel — must
/// still exit `TurnOutcome::Cancelled` instead of running to
/// completion.
#[tokio::test]
async fn cancel_racing_turn_startup_cancels_in_flight_turn() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    // Slow resolver so the cancel can be submitted while the turn is
    // parked in `run_turn_inner`'s resolution phase.
    let resolver = Arc::new(DelayedResolver {
        inner: StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({
                "temp_root": workspace.path().to_string_lossy().to_string()
            }),
            llm_provider: stub_llm_provider(),
            cancel_token: None,
        },
        delay: std::time::Duration::from_millis(300),
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .open_session(test_open_request("s-race-cancel"))
        .await
        .expect("open session");

    // Submit the turn first: it stamps its submission time, then parks
    // in the resolver's delay.
    let service = Arc::clone(&dependencies.session_service);
    let turn = tokio::spawn(async move {
        service
            .run_turn(test_open_request("s-race-cancel").into_turn_request("hi".to_string()))
            .await
            .expect("turn should complete")
    });
    // Give the spawned task a scheduling slot so its submission is
    // stamped before the cancel is recorded.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Esc while the turn is in flight: the actor sees no active turn
    // and records the cancel's arrival time.
    let receipt = dependencies
        .session_control_plane
        .submit_input("s-race-cancel", SessionInput::CancelActiveTurn)
        .await
        .expect("cancel should be accepted");
    assert_eq!(receipt.accepted_kind, SessionInputKind::CancelActiveTurn);

    // The turn was submitted before the cancel: it starts with its
    // cancel token already fired and terminates `Cancelled`.
    let result = turn.await.expect("turn task must not panic");
    assert_eq!(result.outcome, TurnOutcome::Cancelled);
}

#[tokio::test]
async fn force_close_session_removes_session_record() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .open_session(SessionOpenRequest {
            session_id: "s-close".to_string(),
            conversation_id: "c1".to_string(),
            sender_id: "u1".to_string(),
            entry: GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
            llm: None,
            workspace: None,
            skills: None,

            client_id: None,
            client_pid: None,
            client_hostname: None,
        })
        .await
        .expect("open session");

    let closed = dependencies
        .session_control_plane
        .force_close_session("s-close")
        .await
        .expect("close session");
    assert_eq!(closed.status, SessionLifecycleStatus::Closed);

    let resumed = dependencies
        .session_control_plane
        .resume_session("s-close")
        .await
        .expect("resume closed session");
    assert!(resumed.is_none());
}

#[tokio::test]
async fn pause_runtime_releases_backend_and_marks_runtime_paused() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let backend_manager = Arc::new(BackendManager::new());
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        backend_manager.clone(),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .open_session(test_open_request("runtime-pause"))
        .await
        .expect("open session");

    let paused = dependencies
        .session_control_plane
        .pause_runtime(RuntimePauseRequest {
            runtime_id: "runtime-pause".to_string(),
            metadata: Value::Null,
            name: Some("pause".to_string()),

            client_id: None,
        })
        .await
        .expect("pause runtime");

    assert_eq!(paused.runtime.runtime_id, "runtime-pause");
    assert_eq!(paused.runtime.status, SessionLifecycleStatus::Paused);
    let saved = store.load("runtime-pause").await.expect("saved runtime");
    assert_eq!(saved.status, SessionLifecycleStatus::Paused);
    assert!(saved.backend_instance.is_none());
    assert!(matches!(
        backend_manager.lease_bound_session("runtime-pause").await,
        Err(BackendError::NotFound { .. })
    ));
}

#[tokio::test]
async fn resume_runtime_reuses_same_runtime_id_for_paused_runtime_without_backend() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    save_session_without_backend(
        &store,
        &resolver,
        "runtime-resume",
        SessionLifecycleStatus::Idle,
    )
    .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    dependencies
        .session_control_plane
        .pause_runtime(RuntimePauseRequest {
            runtime_id: "runtime-resume".to_string(),
            metadata: Value::Null,
            name: None,

            client_id: None,
        })
        .await
        .expect("pause runtime");
    let resumed = dependencies
        .session_control_plane
        .resume_runtime(RuntimeResumeRequest {
            runtime_id: "runtime-resume".to_string(),
            metadata: Value::Null,

            client_id: None,
        })
        .await
        .expect("resume runtime");

    assert_eq!(resumed.runtime.runtime_id, "runtime-resume");
    assert_eq!(resumed.runtime.status, SessionLifecycleStatus::Idle);
    let saved = store.load("runtime-resume").await.expect("saved runtime");
    assert_eq!(saved.status, SessionLifecycleStatus::Idle);
    assert!(saved.backend_instance.is_none());
}

#[tokio::test]
async fn close_paused_runtime_removes_session_record() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    save_session_without_backend(
        &store,
        &resolver,
        "runtime-close-paused",
        SessionLifecycleStatus::Paused,
    )
    .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let closed = dependencies
        .session_control_plane
        .force_close_session("runtime-close-paused")
        .await
        .expect("close paused runtime");

    assert_eq!(closed.status, SessionLifecycleStatus::Closed);
    assert!(store.load("runtime-close-paused").await.is_none());
}

#[tokio::test]
async fn run_turn_paused_runtime_without_checkpoint_is_busy() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    save_session_without_backend(
        &store,
        &resolver,
        "runtime-paused-turn",
        SessionLifecycleStatus::Paused,
    )
    .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let result = dependencies
        .session_service
        .run_turn(test_open_request("runtime-paused-turn").into_turn_request("hi".to_string()))
        .await;

    // A paused session without an eviction checkpoint cannot be resumed
    // and must be reported as busy rather than silently rejected.
    assert!(matches!(
        result,
        Err(SessionServiceError::SessionBusy { message, .. })
            if message.contains("no eviction checkpoint")
    ));
}

#[tokio::test]
async fn hibernated_mcp_runtime_keeps_record_and_rehydrates_local_backend() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let backend_manager = Arc::new(BackendManager::new());
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        backend_manager.clone(),
    )
    .expect("dependencies");
    let mut open = test_open_request("mcp-hibernate");
    open.entry = GatewayEntryContext {
        kind: Some(crate::gateway::GatewayEntryKind::Mcp),
        instance_id: Some("chatbot".to_string()),
        runtime_profile_id: None,
        build_tags: Vec::new(),
    };
    dependencies
        .session_control_plane
        .open_session(open.clone())
        .await
        .expect("open MCP session");

    let paused = dependencies
        .session_control_plane
        .hibernate_idle_session("mcp-hibernate", u64::MAX)
        .await
        .expect("hibernate")
        .expect("idle session should hibernate");
    assert_eq!(paused.status, SessionLifecycleStatus::Paused);
    assert!(paused.backend_instance.is_none());
    let saved = store.load("mcp-hibernate").await.expect("record retained");
    assert_eq!(saved.status, SessionLifecycleStatus::Paused);
    assert!(matches!(
        backend_manager.lease_bound_session("mcp-hibernate").await,
        Err(BackendError::NotFound { .. })
    ));

    let result = dependencies
        .session_service
        .run_turn(open.into_turn_request("continue".to_string()))
        .await;
    assert!(
        !matches!(
            result,
            Err(SessionServiceError::SessionBusy { ref message, .. })
                if message.contains("no eviction checkpoint")
        ),
        "hibernated MCP local sessions must rebuild instead of requiring a checkpoint"
    );
}

#[tokio::test]
async fn checkpoint_runtime_idle_session_without_backend_succeeds() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    save_session_without_backend(&store, &resolver, "runtime-1", SessionLifecycleStatus::Idle)
        .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let result = dependencies
        .session_control_plane
        .checkpoint_runtime(RuntimeCheckpointRequest {
            runtime_id: "runtime-1".to_string(),
            metadata: json!({"kind": "test"}),
            name: Some("checkpoint-a".to_string()),

            client_id: None,
        })
        .await
        .expect("checkpoint runtime");

    assert!(result.checkpoint_id.starts_with("rtcp_"));
    assert_eq!(result.runtime.runtime_id, "runtime-1");
    assert_eq!(result.parent_checkpoint_id, None);
    assert_eq!(result.metadata, json!({"kind": "test"}));
    assert_eq!(result.name.as_deref(), Some("checkpoint-a"));
}

#[tokio::test]
async fn delete_checkpoint_snapshot_without_backend_snapshot_is_noop() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    save_session_without_backend(&store, &resolver, "runtime-1", SessionLifecycleStatus::Idle)
        .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");
    let checkpoint = dependencies
        .session_control_plane
        .checkpoint_runtime(RuntimeCheckpointRequest {
            runtime_id: "runtime-1".to_string(),
            metadata: Value::Null,
            name: None,

            client_id: None,
        })
        .await
        .expect("checkpoint runtime");

    let result = dependencies
        .session_control_plane
        .delete_checkpoint_snapshot(RuntimeCheckpointSnapshotDeleteRequest {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
        })
        .await
        .expect("delete checkpoint snapshot");

    assert_eq!(result.checkpoint_id, checkpoint.checkpoint_id);
    assert_eq!(result.runtime_id, "runtime-1");
    assert_eq!(result.provider, None);
    assert_eq!(result.provider_snapshot_id, None);
    assert!(!result.deleted_provider_snapshot);
}

#[tokio::test]
async fn checkout_runtime_creates_new_runtime_from_checkpoint() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let parent = save_session_without_backend(
        &store,
        &resolver,
        "runtime-parent",
        SessionLifecycleStatus::Idle,
    )
    .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");
    let checkpoint = dependencies
        .session_control_plane
        .checkpoint_runtime(RuntimeCheckpointRequest {
            runtime_id: parent.session_id.clone(),
            metadata: Value::Null,
            name: None,

            client_id: None,
        })
        .await
        .expect("checkpoint runtime");

    let checkout = dependencies
        .session_control_plane
        .checkout_runtime(RuntimeCheckoutRequest {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            conversation_id: Some("child-conversation".to_string()),
            sender_id: Some("child-user".to_string()),
            metadata: json!({"branch": "a"}),
            options: None,
        })
        .await
        .expect("checkout runtime");

    assert_eq!(checkout.checkpoint_id, checkpoint.checkpoint_id);
    assert_eq!(checkout.source_runtime_id, parent.session_id);
    assert_ne!(checkout.runtime.runtime_id, parent.session_id);
    assert!(checkout
        .runtime
        .runtime_id
        .starts_with("runtime-parent:checkout:"));
    assert_eq!(checkout.runtime.conversation_id, "child-conversation");
    assert_eq!(checkout.runtime.sender_id, "child-user");

    let saved = store
        .load(&checkout.runtime.runtime_id)
        .await
        .expect("checked out runtime saved");
    assert_eq!(saved.conversation_id, "child-conversation");
    assert_eq!(saved.sender_id, "child-user");
    assert_eq!(saved.status, SessionLifecycleStatus::Idle);
    assert!(saved.backend_instance.is_none());
    assert_eq!(saved.runtime.agent_id, parent.runtime.agent_id);
}

#[tokio::test]
async fn checkpoint_runtime_rejects_running_session() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    save_session_without_backend(
        &store,
        &resolver,
        "runtime-running",
        SessionLifecycleStatus::Running,
    )
    .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store,
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    let result = dependencies
        .session_control_plane
        .checkpoint_runtime(RuntimeCheckpointRequest {
            runtime_id: "runtime-running".to_string(),
            metadata: Value::Null,
            name: None,

            client_id: None,
        })
        .await;

    assert!(matches!(
        result,
        Err(SessionServiceError::SessionBusy { session_id, .. })
            if session_id == "runtime-running"
    ));
}

#[tokio::test]
async fn checkout_runtime_rejects_running_source_runtime() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let mut parent = save_session_without_backend(
        &store,
        &resolver,
        "runtime-source",
        SessionLifecycleStatus::Idle,
    )
    .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver,
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");
    let checkpoint = dependencies
        .session_control_plane
        .checkpoint_runtime(RuntimeCheckpointRequest {
            runtime_id: "runtime-source".to_string(),
            metadata: Value::Null,
            name: None,

            client_id: None,
        })
        .await
        .expect("checkpoint runtime");
    parent.status = SessionLifecycleStatus::Running;
    store.save(parent).await;

    let result = dependencies
        .session_control_plane
        .checkout_runtime(RuntimeCheckoutRequest {
            checkpoint_id: checkpoint.checkpoint_id,
            conversation_id: None,
            sender_id: None,
            metadata: Value::Null,
            options: None,
        })
        .await;

    assert!(matches!(
        result,
        Err(SessionServiceError::SessionBusy { session_id, .. })
            if session_id == "runtime-source"
    ));
}

#[tokio::test]
async fn force_close_session_cascades_to_descendants() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let parent =
        save_session_without_backend(&store, &resolver, "rt-parent", SessionLifecycleStatus::Idle)
            .await;
    let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
        store.clone(),
        resolver.clone(),
        HookerRegistryConfig::default(),
        Arc::new(BackendManager::new()),
    )
    .expect("dependencies");

    // parent -> checkpoint -> child
    let checkpoint = dependencies
        .session_control_plane
        .checkpoint_runtime(RuntimeCheckpointRequest {
            runtime_id: parent.session_id.clone(),
            metadata: Value::Null,
            name: None,

            client_id: None,
        })
        .await
        .expect("checkpoint runtime");
    let checkout = dependencies
        .session_control_plane
        .checkout_runtime(RuntimeCheckoutRequest {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            conversation_id: Some("child-conversation".to_string()),
            sender_id: Some("child-user".to_string()),
            metadata: json!({"branch": "a"}),
            options: None,
        })
        .await
        .expect("checkout runtime");
    let child_id = checkout.runtime.runtime_id.clone();
    assert!(store.load(&child_id).await.is_some());

    // Manually register a grandchild whose parent_runtime_id points at
    // the child, to exercise recursive cascade without a second
    // checkpoint/checkout round-trip.
    let mut grandchild = save_session_without_backend(
        &store,
        &resolver,
        "rt-grandchild",
        SessionLifecycleStatus::Idle,
    )
    .await;
    grandchild.parent_runtime_id = Some(child_id.clone());
    store.save(grandchild.clone()).await;
    assert!(store.load("rt-grandchild").await.is_some());

    // Closing the parent must cascade-close child and grandchild.
    dependencies
        .session_control_plane
        .force_close_session(&parent.session_id)
        .await
        .expect("force close parent");

    assert!(
        store.load("rt-parent").await.is_none(),
        "parent should be deleted"
    );
    assert!(
        store.load(&child_id).await.is_none(),
        "child should be cascade-deleted"
    );
    assert!(
        store.load("rt-grandchild").await.is_none(),
        "grandchild should be cascade-deleted"
    );

    // The checkpoint record for the parent must also be gone, so a
    // subsequent checkout from it fails with SessionNotFound.
    let result = dependencies
        .session_control_plane
        .checkout_runtime(RuntimeCheckoutRequest {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            conversation_id: None,
            sender_id: None,
            metadata: Value::Null,
            options: None,
        })
        .await;
    assert!(
        matches!(result, Err(SessionServiceError::SessionNotFound { .. })),
        "checkpoint should have been removed during close, got: {:?}",
        result
    );
}

// ---------- Orphan-session reaper tests ----------
//
// Tests construct `CoreBackedSessionService` directly (via `use super::*`)
// so they can call the private `run_reaper_sweep` and inspect the inner
// lease table. The four reaper conditions (Closed / live lease / recent
// activity / Running handle) are exercised independently; the Running-
// handle case needs the full bootstrap and is covered by the close-cascade
// test above (the `mark_closing` race-fix by the handle's own re-queue
// path in `next_runnable_turn`).

/// Build a `StubRuntimeResolver` whose `backend_options` points
/// `temp_root` at `workspace`, so the stub backend writes into the
/// test's tempdir instead of the real workspace. Centralises the
/// `{ workspace_root, backend_options, llm_provider }` triple that was
/// copy-pasted across every reaper / checkpoint / fork test.
fn make_stub_resolver(workspace: &std::path::Path) -> Arc<StubRuntimeResolver> {
    Arc::new(StubRuntimeResolver {
        workspace_root: workspace.to_path_buf(),
        backend_options: json!({"temp_root": workspace.to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    })
}

fn make_reaper_service(
    store: Arc<InMemorySessionStore>,
    resolver: Arc<StubRuntimeResolver>,
) -> Arc<CoreBackedSessionService> {
    let hooker_registry = HookerRegistryBuilderImpl::new()
        .with_config(HookerRegistryConfig::default())
        .build()
        .expect("build hooker registry");
    // The chain-depth cap is irrelevant to the reaper; reuse the
    // documented default of 128 (see `DEFAULT_MAX_PROMPT_CHAIN_DEPTH`
    // in `agent_types::hook`).
    Arc::new(CoreBackedSessionService::new(
        store,
        resolver,
        Arc::from(hooker_registry),
        Arc::new(BackendManager::new()),
        128,
        None,
    ))
}

/// Save a session record with no leased backend, controlling
/// `updated_at_ms` directly. Used by the reaper tests so we can
/// backdate activity past `ORPHAN_SESSION_THRESHOLD_MS` without waiting.
async fn save_orphan_candidate(
    store: &Arc<InMemorySessionStore>,
    resolver: &Arc<StubRuntimeResolver>,
    session_id: &str,
    status: SessionLifecycleStatus,
    updated_at_ms: u64,
) -> SessionRecord {
    let request = test_open_request(session_id);
    let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
    let resolved = resolver
        .resolve(&runtime_input, None)
        .await
        .expect("resolve runtime");
    let mut session = CoreBackedSessionService::build_session_for_open(&request, &resolved);
    session.status = status;
    session.backend_instance = None;
    session.updated_at_ms = updated_at_ms;
    store.save(session.clone()).await;
    session
}

#[tokio::test]
async fn reaper_skips_session_with_live_lease() {
    // The reaper's most important skip: a stale `updated_at_ms` is NOT
    // enough to reap when a live lease exists. Without this guard the
    // reaper would close a session out from under its current holder.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());

    save_orphan_candidate(
        &store,
        &resolver,
        "rt-held",
        SessionLifecycleStatus::Idle,
        crate::gateway::session_lease::current_time_ms()
            .expect("wall clock")
            .saturating_sub(ORPHAN_SESSION_THRESHOLD_MS + 1_000),
    )
    .await;
    let _ = service
        .sessions_lease
        .acquire("rt-held", "client-a", Some(1), None)
        .await;

    service
        .run_reaper_sweep(ORPHAN_SESSION_THRESHOLD_MS, STALE_LEASE_THRESHOLD_MS)
        .await
        .expect("reaper sweep");

    assert!(
        store.load("rt-held").await.is_some(),
        "session with a live lease must not be reaped"
    );
    let snapshot = service.sessions_lease.snapshot().await;
    assert!(snapshot.iter().any(|(sid, _, _)| sid == "rt-held"));
}

#[tokio::test]
async fn reaper_closes_stale_orphan_session() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());

    // No lease, stale activity, Idle status — the textbook orphan.
    save_orphan_candidate(
        &store,
        &resolver,
        "rt-orphan",
        SessionLifecycleStatus::Idle,
        crate::gateway::session_lease::current_time_ms()
            .expect("wall clock")
            .saturating_sub(ORPHAN_SESSION_THRESHOLD_MS + 1_000),
    )
    .await;

    service
        .run_reaper_sweep(ORPHAN_SESSION_THRESHOLD_MS, STALE_LEASE_THRESHOLD_MS)
        .await
        .expect("reaper sweep");

    // The reaper force-closes the session: store record deleted, lease
    // table entry (if any) also removed.
    assert!(
        store.load("rt-orphan").await.is_none(),
        "stale orphan session must be reaped and removed from the store"
    );
    let snapshot = service.sessions_lease.snapshot().await;
    assert!(
        snapshot.iter().all(|(sid, _, _)| sid != "rt-orphan"),
        "lease table entry for reaped session must be cleared"
    );
}

#[tokio::test]
async fn reaper_force_closes_session_with_real_idle_handle() {
    // Exercises the reaper's step-4 `Some(handle)` branch (an in-memory
    // Idle handle → `mark_closing`, not skipped). Opens a REAL session
    // so a `SessionHandle` + actor exist, then backdates the store
    // record past the orphan threshold so the reaper force-closes it
    // through the full `force_close_session_inner` success path.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());

    // Anonymous open (client_id = None) bypasses lease acquire, so the
    // reaper's step-2 "live lease" check won't skip — we want to reach
    // step 4.
    service
        .open_session(test_open_request("rt-idle-handle"))
        .await
        .expect("open session");

    // Verify the in-memory handle exists so we know step 4's `Some(handle)`
    // branch is exercised (not the no-handle `unwrap_or(true)` path).
    assert!(
        service
            .sessions_handler
            .lock()
            .await
            .get("rt-idle-handle")
            .is_some(),
        "handle must exist before reaping (else the test exercises the wrong branch)"
    );

    // Backdate the store record past the orphan threshold. `open_session`
    // stamped `updated_at_ms = now`; overwrite it so the reaper's step 3
    // (stale activity) doesn't skip.
    {
        let mut record = store
            .load("rt-idle-handle")
            .await
            .expect("record present after open");
        record.updated_at_ms = crate::gateway::session_lease::current_time_ms()
            .expect("wall clock")
            .saturating_sub(ORPHAN_SESSION_THRESHOLD_MS + 1_000);
        store.save(record).await;
    }

    service
        .run_reaper_sweep(ORPHAN_SESSION_THRESHOLD_MS, STALE_LEASE_THRESHOLD_MS)
        .await
        .expect("reaper sweep");

    // The reaper force-closed the session: store record deleted and
    // in-memory handle evicted (step 6 of `force_close_session_inner`).
    assert!(
        store.load("rt-idle-handle").await.is_none(),
        "session with a real Idle handle + stale activity must be reaped"
    );
    assert!(
        service
            .sessions_handler
            .lock()
            .await
            .get("rt-idle-handle")
            .is_none(),
        "in-memory handle must be evicted after reaping"
    );
}

#[tokio::test]
async fn reaper_evicts_artifacts_when_close_fails_on_missing_record() {
    // Error-path cleanup: when `force_close_session_inner` returns `Err`
    // (here: `SessionNotFound` because the store record was deleted
    // between `list_all()` snapshot and `reap_one_record`), the reaper
    // calls `evict_session_artifacts` so the session is not bricked.
    //
    // We drive `reap_one_record` directly with a stale snapshot of a
    // record whose store entry has already been deleted, simulating a
    // concurrent close.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());

    // Save a stale orphan candidate and capture its snapshot.
    let record = save_orphan_candidate(
        &store,
        &resolver,
        "rt-missing",
        SessionLifecycleStatus::Idle,
        crate::gateway::session_lease::current_time_ms()
            .expect("wall clock")
            .saturating_sub(ORPHAN_SESSION_THRESHOLD_MS + 1_000),
    )
    .await;

    // Simulate a concurrent close that deleted the store record BETWEEN
    // the reaper's `list_all()` snapshot and `reap_one_record`. Now
    // `force_close_session_inner` → `close_self_record` →
    // `session_store.load` returns None → `SessionNotFound`.
    store.delete("rt-missing").await;

    // Drive `reap_one_record` directly with the stale snapshot. The reaper
    // must NOT panic; the error path calls `evict_session_artifacts`
    // (idempotent on absent keys) so the store stays clean.
    let now = crate::gateway::session_lease::current_time_ms().expect("wall clock");
    service
        .reap_one_record(
            &record,
            now,
            &[],
            ORPHAN_SESSION_THRESHOLD_MS,
            STALE_LEASE_THRESHOLD_MS,
        )
        .await;

    // Store must remain clean (the record was already deleted; evict is
    // a no-op on absent keys — proving the error path is exercised
    // without panicking or leaving partial state).
    assert!(
        store.load("rt-missing").await.is_none(),
        "store must remain clean after error-path cleanup"
    );
    // No handle was ever created, so none should linger.
    assert!(
        service
            .sessions_handler
            .lock()
            .await
            .get("rt-missing")
            .is_none(),
        "no handle should linger after error-path cleanup"
    );
}

// ---------- SessionControlPlane lease trait methods ----------
//
// `assert_lease_holder` / `heartbeat_session` / `detach_session` /
// `force_close_session_with_lease` implement the single-writer guarantee
// at the service layer. `session_lease::tests` covers the underlying
// `SessionLeaseTable`; these pin the service-layer contract (including
// the `enforce_anonymous_lease` toggle and the daemon-principal bypass).

async fn make_lease_service() -> Arc<CoreBackedSessionService> {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    make_reaper_service(store, resolver)
}

#[tokio::test]
async fn assert_lease_holder_allows_anonymous_when_enforce_off() {
    // Default: `enforce_anonymous_lease = false`. An anonymous caller
    // (`None`) bypasses the check, matching the gradual-rollout policy.
    let service = make_lease_service().await;
    assert!(!service.anonymous_lease_enforced());
    assert!(
        service
            .assert_lease_holder("any-session", None)
            .await
            .is_ok(),
        "anonymous caller bypasses the holder check when enforcement is off"
    );
}

#[tokio::test]
async fn assert_lease_holder_rejects_anonymous_when_enforce_on() {
    let service = make_lease_service().await;
    service.set_enforce_anonymous_lease(true);
    assert!(service.anonymous_lease_enforced());
    let err = service
        .assert_lease_holder("any-session", None)
        .await
        .expect_err("anonymous caller rejected when enforcement is on");
    assert!(
        matches!(err, SessionServiceError::LeaseRequired { .. }),
        "expected LeaseRequired, got {err:?}"
    );
}

#[tokio::test]
async fn open_session_allows_anonymous_when_enforce_off() {
    // Default: `enforce_anonymous_lease = false`. An anonymous caller
    // (`client_id: None`) bypasses the lease acquire entirely, matching
    // the gradual-rollout policy in `assert_lease_holder`. Build the
    // service with a workspace that outlives the call (unlike
    // `make_lease_service`, whose `TempDir` is dropped on return) so
    // `open_session_inner` can actually lease the backend.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store, resolver);
    assert!(!service.anonymous_lease_enforced());
    let request = test_open_request("rt-open-anon-off");
    service
        .open_session(request)
        .await
        .expect("anonymous caller may open when enforcement is off");
}

#[tokio::test]
async fn open_session_rejects_anonymous_when_enforce_on() {
    // Regression: before the anonymous-policy guard was added to
    // `open_session`, an anonymous HTTP caller could open/resume a
    // session without acquiring the lease even when
    // `XIAOO_ENFORCE_LEASE=on`, because the `if let Some(client_id)`
    // skip path had no `enforce_anonymous_lease` check (unlike
    // `assert_lease_holder` / `force_close_session_with_lease`). The
    // guard now rejects such callers with `LeaseRequired` before the
    // bypass, so `open_session` is no longer the one mutating RPC that
    // defeats strict single-writer enforcement.
    let service = make_lease_service().await;
    service.set_enforce_anonymous_lease(true);
    assert!(service.anonymous_lease_enforced());
    let request = test_open_request("rt-open-anon-on");
    let err = service
        .open_session(request)
        .await
        .expect_err("anonymous caller rejected on open when enforcement is on");
    assert!(
        matches!(err, SessionServiceError::LeaseRequired { .. }),
        "expected LeaseRequired, got {err:?}"
    );
}

#[tokio::test]
async fn assert_lease_holder_rejects_non_holder() {
    // client-a acquires; client-b's check must fail with
    // `SessionAttachedByAnotherClient` (the structured-error contract
    // the TUI parses in `parse_session_attached_body`).
    let service = make_lease_service().await;
    let _ = service
        .sessions_lease
        .acquire("s1", "client-a", Some(1), None)
        .await;
    let err = service
        .assert_lease_holder("s1", Some("client-b"))
        .await
        .expect_err("non-holder must be rejected");
    match err {
        SessionServiceError::SessionAttachedByAnotherClient {
            holder_client_id,
            stale: false,
            ..
        } => assert_eq!(holder_client_id, "client-a"),
        other => panic!("expected SessionAttachedByAnotherClient, got {other:?}"),
    }
}

#[tokio::test]
async fn assert_lease_holder_bypasses_for_daemon_principal() {
    // Daemon-internal principals (cron / hook / channel) bypass the
    // holder check explicitly so post-turn hooks and scheduled jobs
    // don't fail when an HTTP client holds the lease.
    let service = make_lease_service().await;
    let _ = service
        .sessions_lease
        .acquire("s1", "client-a", Some(1), None)
        .await;
    let cron_id = crate::gateway::daemon_cron_principal();
    assert!(
        service
            .assert_lease_holder("s1", Some(&cron_id))
            .await
            .is_ok(),
        "daemon:cron must bypass the holder check"
    );
}

#[tokio::test]
async fn heartbeat_session_no_op_for_anonymous_caller() {
    // Default enforcement is off → anonymous caller is allowed through
    // without touching the lease table.
    let service = make_lease_service().await;
    assert!(service
        .heartbeat_session(crate::gateway::SessionHeartbeatRequest {
            session_id: "s1".to_string(),
            client_id: None,
            client_pid: None,
            client_hostname: None,
        })
        .await
        .is_ok());
}

#[tokio::test]
async fn force_close_session_with_lease_rejects_non_holder() {
    // client-a holds; client-b's close is rejected with
    // `SessionAttachedByAnotherClient` *before* the session store is even
    // touched, so no real session record is needed — only the lease.
    let service = make_lease_service().await;
    let _ = service
        .sessions_lease
        .acquire("rt-held", "client-a", Some(1), None)
        .await;
    let err = service
        .force_close_session_with_lease("rt-held", Some("client-b"))
        .await
        .expect_err("non-holder close must be rejected");
    assert!(
        matches!(
            err,
            SessionServiceError::SessionAttachedByAnotherClient { .. }
        ),
        "expected SessionAttachedByAnotherClient, got {err:?}"
    );
}

#[tokio::test]
async fn force_close_session_with_lease_daemon_principal_bypasses_check() {
    // daemon:cron can close a session even though client-a holds the
    // lease (matches the trait-level bypass used by cron / hook / channel
    // ingress). The session's lease is dropped as part of the close.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());
    let _ = service
        .sessions_lease
        .acquire("rt-daemon", "client-a", Some(1), None)
        .await;
    save_orphan_candidate(
        &store,
        &resolver,
        "rt-daemon",
        SessionLifecycleStatus::Idle,
        crate::gateway::session_lease::current_time_ms().expect("wall clock"),
    )
    .await;
    let cron_id = crate::gateway::daemon_cron_principal();
    let _ = service
        .force_close_session_with_lease("rt-daemon", Some(&cron_id))
        .await
        .expect("daemon principal close succeeds");
    assert!(store.load("rt-daemon").await.is_none());
}

// ---------- SessionActor pop-time lease rejection ----------
//
// `next_runnable_turn` is the LAST line of defense for single-writer: the
// router-level `assert_lease_holder` is the first, but a turn queued
// before a takeover waits in `pending_turns` while the lease changes
// hands; the actor's pop-time check rejects it with
// `SessionAttachedByAnotherClient` instead of starting it mid-stream.

async fn open_real_session_with_client(
    service: &Arc<CoreBackedSessionService>,
    session_id: &str,
    client_id: &str,
) {
    let request = SessionOpenRequest {
        session_id: session_id.to_string(),
        conversation_id: format!("{session_id}-conversation"),
        sender_id: "test-user".to_string(),
        entry: GatewayEntryContext::tui(None),
        channel: None,
        channel_instance_id: None,
        llm: None,
        workspace: None,
        skills: None,
        client_id: Some(client_id.to_string()),
        client_pid: Some(1),
        client_hostname: Some("test-host".to_string()),
    };
    service
        .open_session(request)
        .await
        .expect("open session with client_id");
}

#[tokio::test]
async fn pop_time_check_rejects_turn_after_lease_takeover() {
    // Open a real session with client-a (→ client-a holds the lease, an
    // actor is running); backdate client-a's lease so client-b can take
    // it over via the stale path; then submit a turn whose `client_id`
    // is still `client-a`. The actor's pop-time check must reject with
    // `SessionAttachedByAnotherClient` instead of running the turn.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());
    open_real_session_with_client(&service, "rt-pop", "client-a").await;

    // Backdate client-a's lease and let client-b take it over via the
    // stale path (live leases are never overridden).
    service
        .sessions_lease
        .set_last_heartbeat_ms_for_test(
            "rt-pop",
            crate::gateway::session_lease::current_time_ms()
                .expect("wall clock")
                .saturating_sub(STALE_LEASE_THRESHOLD_MS + 1_000),
        )
        .await;
    let _ = service
        .sessions_lease
        .acquire("rt-pop", "client-b", Some(2), None)
        .await;

    // Submit a turn on behalf of client-a (the now-stale holder). The
    // actor should reject it before invoking the LLM.
    let mut request = test_open_request("rt-pop").into_turn_request("hi".to_string());
    request.client_id = Some("client-a".to_string());
    let result = service.run_turn(request).await;
    match result {
        Err(SessionServiceError::SessionAttachedByAnotherClient {
            holder_client_id, ..
        }) => assert_eq!(holder_client_id, "client-b"),
        other => {
            panic!("expected SessionAttachedByAnotherClient (holder=client-b), got {other:?}")
        }
    }
}

#[tokio::test]
async fn pop_time_check_allows_daemon_principal_when_http_client_holds_lease() {
    // A daemon-internal principal (cron / hook / channel) bypasses the
    // pop-time check even when an HTTP client holds the lease. Without
    // this bypass, a scheduled cron job firing while a TUI is attached
    // would be rejected and the cron would never run.
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = make_stub_resolver(workspace.path());
    let service = make_reaper_service(store.clone(), resolver.clone());
    open_real_session_with_client(&service, "rt-cron", "client-a").await;

    // Daemon-internal principal's turn is allowed through (the LLM may
    // or may not succeed depending on stub config, but the actor must
    // NOT reject it with SessionAttachedByAnotherClient — that's the
    // contract being tested).
    let mut request = test_open_request("rt-cron").into_turn_request("hi".to_string());
    request.client_id = Some(crate::gateway::daemon_cron_principal());
    let result = service.run_turn(request).await;
    // We only assert the rejection contract here — the turn itself may
    // fail for stub-backend reasons (e.g. LLM provider config). The
    // important invariant: it's NOT `SessionAttachedByAnotherClient`.
    if let Err(SessionServiceError::SessionAttachedByAnotherClient { .. }) = result {
        panic!("daemon:cron turn must NOT be rejected by the pop-time holder check");
    }
}

// ===== session lifecycle state hook (idle / failed) =====
//
// The `failed` integration path through `run_turn` is not exercised:
// reliably forcing `run_turn_inner` to return `Err` requires a
// specific LLM/backend failure mode outside this crate's contract.

/// Records every `*.Session.lifecycle.state` invocation as a
/// `(state, outcome, workspace)` triple onto a shared `Vec`, optionally
/// returning per-state `actions`. Uses the `*` hook point so the test
/// doesn't need to know the resolver's agent_id.
struct RecordingStateHooker {
    id: HookerId,
    hook_point: HookPointId,
    invocations: Arc<StdMutex<Vec<(String, String, Option<String>)>>>,
    actions_by_state: HashMap<String, Vec<HookAction>>,
}

impl RecordingStateHooker {
    fn new(
        id: &str,
        invocations: Arc<StdMutex<Vec<(String, String, Option<String>)>>>,
        actions_by_state: HashMap<String, Vec<HookAction>>,
    ) -> Self {
        Self {
            id: HookerId(id.to_string()),
            hook_point: HookPointId("*.Session.lifecycle.state".to_string()),
            invocations,
            actions_by_state,
        }
    }
}

#[async_trait]
impl Hooker for RecordingStateHooker {
    fn id(&self) -> &HookerId {
        &self.id
    }

    fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        _runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        match input {
            HookInvokeInput::SessionState {
                input: state_input, ..
            } => {
                self.invocations
                    .lock()
                    .expect("recording hooker invocations")
                    .push((
                        state_input.state.clone(),
                        state_input.outcome.clone(),
                        state_input.workspace.clone(),
                    ));
                let actions = self
                    .actions_by_state
                    .get(&state_input.state)
                    .cloned()
                    .unwrap_or_default();
                Ok(
                    HookInvokeOutput::SessionState(SessionHookResult::Acknowledged)
                        .with_actions(actions),
                )
            }
            other => Err(HookInvokeError::Session(SessionHookError::Plugin {
                message: format!(
                    "recording hooker '{}' expected SessionState input but got {:?}",
                    self.id.0, other
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Build a `HookerRegistry` with a single enabled `RecordingStateHooker`
/// for `*.Session.lifecycle.state`.
fn recording_state_hooker_registry(
    invocations: Arc<StdMutex<Vec<(String, String, Option<String>)>>>,
    actions_by_state: HashMap<String, Vec<HookAction>>,
) -> Arc<dyn HookerRegistry> {
    let hooker = Box::new(RecordingStateHooker::new(
        "recording-state-hooker",
        invocations,
        actions_by_state,
    ));
    let hooker_id = hooker.id().clone();
    let mut hookers: HashMap<HookerId, Box<dyn Hooker>> = HashMap::new();
    hookers.insert(hooker_id.clone(), hooker);
    let mut enabled: HashSet<HookerId> = HashSet::new();
    enabled.insert(hooker_id);
    Arc::new(HookerRegistryImpl::new(hookers, enabled, HashMap::new()))
}

/// Construct a `CoreBackedSessionService` with an explicit hooker
/// registry, mirroring `make_reaper_service` but exposing the registry
/// argument so tests can inject a recording hooker.
fn build_service_with_hooker_registry(
    store: Arc<InMemorySessionStore>,
    resolver: Arc<StubRuntimeResolver>,
    hooker_registry: Arc<dyn HookerRegistry>,
) -> Arc<CoreBackedSessionService> {
    Arc::new(CoreBackedSessionService::new(
        store,
        resolver,
        hooker_registry,
        Arc::new(BackendManager::new()),
        128,
        None,
    ))
}

/// Poll `invocations` until it holds `expected` entries, or panic
/// after `timeout_ms` (the caller cannot await the fire-and-forget
/// background task).
async fn wait_for_invocations<T>(
    invocations: &Arc<StdMutex<Vec<T>>>,
    expected: usize,
    timeout_ms: u64,
) -> Vec<T>
where
    T: Clone,
{
    tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        loop {
            let snapshot = invocations.lock().expect("invocations").clone();
            if snapshot.len() >= expected {
                return snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("timed out waiting for hooker invocations")
}

#[tokio::test]
async fn fire_session_state_hook_background_dispatches_failed_state() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let invocations = Arc::new(StdMutex::new(Vec::new()));
    let registry = recording_state_hooker_registry(invocations.clone(), HashMap::new());
    let service = build_service_with_hooker_registry(store, resolver, registry);

    service.fire_session_state_hook_background(SessionStateHookEvent {
        session_id: "session-failed".to_string(),
        sender_id: "user-1".to_string(),
        agent_id: "test-agent".to_string(),
        state: SessionLifecycleStatus::Failed.as_tag().to_string(),
        outcome: SessionStateOutcome::Error.as_tag().to_string(),
        workspace: Some("/ws/background".to_string()),
    });

    let recorded = wait_for_invocations(&invocations, 1, 2_000).await;
    assert_eq!(
        recorded,
        vec![(
            "failed".to_string(),
            "error".to_string(),
            Some("/ws/background".to_string())
        )],
        "fire_session_state_hook_background(state=\"failed\") must dispatch \
             to the hooker with outcome=\"error\" and the event workspace"
    );
}

// ===== session lifecycle hooks: workspace flows from the session record =====

/// Records `*.Session.lifecycle.created` / `closed` invocations as
/// `(session_id, sender_id, workspace)` triples onto a shared `Vec`.
/// `stage` selects which event the hooker registers for; the hook point
/// uses the `*` wildcard so the test doesn't need to know the resolver's
/// agent_id.
struct RecordingLifecycleHooker {
    id: HookerId,
    hook_point: HookPointId,
    invocations: Arc<StdMutex<Vec<(String, String, Option<String>)>>>,
}

impl RecordingLifecycleHooker {
    fn new(
        id: &str,
        stage: &'static str,
        invocations: Arc<StdMutex<Vec<(String, String, Option<String>)>>>,
    ) -> Self {
        Self {
            id: HookerId(id.to_string()),
            hook_point: HookPointId(format!("*.Session.lifecycle.{stage}")),
            invocations,
        }
    }

    fn registry(hooker: Self) -> Arc<dyn HookerRegistry> {
        let hooker_id = hooker.id().clone();
        let mut hookers: HashMap<HookerId, Box<dyn Hooker>> = HashMap::new();
        hookers.insert(hooker_id.clone(), Box::new(hooker));
        let mut enabled: HashSet<HookerId> = HashSet::new();
        enabled.insert(hooker_id);
        Arc::new(HookerRegistryImpl::new(hookers, enabled, HashMap::new()))
    }
}

#[async_trait]
impl Hooker for RecordingLifecycleHooker {
    fn id(&self) -> &HookerId {
        &self.id
    }

    fn hook_point(&self) -> &HookPointId {
        &self.hook_point
    }

    async fn invoke(
        &self,
        input: HookInvokeInput,
        _runtime: &dyn RuntimeView,
    ) -> Result<HookInvokeOutput, HookInvokeError> {
        let record = |session_id: String, sender_id: String, workspace: Option<String>| {
            self.invocations
                .lock()
                .expect("recording hooker invocations")
                .push((session_id, sender_id, workspace));
        };
        match input {
            HookInvokeInput::SessionCreated { input, .. } => {
                record(input.session_id, input.sender_id, input.workspace);
                Ok(HookInvokeOutput::SessionCreated(
                    SessionHookResult::Acknowledged,
                ))
            }
            HookInvokeInput::SessionClosed { input, .. } => {
                record(input.session_id, input.sender_id, input.workspace);
                Ok(HookInvokeOutput::SessionClosed(
                    SessionHookResult::Acknowledged,
                ))
            }
            other => Err(HookInvokeError::Session(SessionHookError::Plugin {
                message: format!(
                    "recording hooker '{}' expected SessionCreated/SessionClosed input but got {:?}",
                    self.id.0, other
                ),
            })),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[tokio::test]
async fn open_session_fires_created_hook_with_workspace() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let invocations = Arc::new(StdMutex::new(Vec::new()));
    let registry = RecordingLifecycleHooker::registry(RecordingLifecycleHooker::new(
        "recording-created-hooker",
        "created",
        invocations.clone(),
    ));
    let service = build_service_with_hooker_registry(store, resolver, registry);

    service
        .open_session(test_open_request("s-created-ws"))
        .await
        .expect("open session");

    let recorded = wait_for_invocations(&invocations, 1, 2_000).await;
    let expected_workspace = workspace.path().to_string_lossy().into_owned();
    assert_eq!(
        recorded,
        vec![(
            "s-created-ws".to_string(),
            "user-1".to_string(),
            Some(expected_workspace)
        )],
        "SessionCreated hook input must carry the session record's workspace root"
    );
}

#[tokio::test]
async fn close_session_fires_closed_hook_with_workspace() {
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: stub_llm_provider(),
        cancel_token: None,
    });
    let invocations = Arc::new(StdMutex::new(Vec::new()));
    let registry = RecordingLifecycleHooker::registry(RecordingLifecycleHooker::new(
        "recording-closed-hooker",
        "closed",
        invocations.clone(),
    ));
    let service = build_service_with_hooker_registry(store, resolver, registry);

    service
        .open_session(test_open_request("s-closed-ws"))
        .await
        .expect("open session");
    service
        .force_close_session("s-closed-ws")
        .await
        .expect("close session");

    let recorded = wait_for_invocations(&invocations, 1, 2_000).await;
    let expected_workspace = workspace.path().to_string_lossy().into_owned();
    assert_eq!(
        recorded,
        vec![(
            "s-closed-ws".to_string(),
            "user-1".to_string(),
            Some(expected_workspace)
        )],
        "SessionClosed hook input must carry the session record's workspace root"
    );
}

#[tokio::test]
async fn run_turn_dispatches_idle_state_hook_with_workspace() {
    // Full-turn path: a successful turn must dispatch `state="idle"` with
    // the workspace taken from the session record (`fire_session_state_hook_and_collect_actions`).
    let workspace = TempDir::new().expect("workspace");
    let store = Arc::new(InMemorySessionStore::default());
    let seen_requests = Arc::new(StdMutex::new(Vec::new()));
    let resolver = Arc::new(StubRuntimeResolver {
        workspace_root: workspace.path().to_path_buf(),
        backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
        llm_provider: replying_llm_provider(Arc::clone(&seen_requests)),
        cancel_token: None,
    });
    let invocations = Arc::new(StdMutex::new(Vec::new()));
    let registry = recording_state_hooker_registry(invocations.clone(), HashMap::new());
    let service = build_service_with_hooker_registry(store, resolver, registry);

    let result = service
        .run_turn(test_open_request("s-idle-ws").into_turn_request("hi".to_string()))
        .await;
    assert!(
        result.is_ok(),
        "turn must succeed for the idle state hook to fire: {result:?}"
    );

    let recorded = wait_for_invocations(&invocations, 1, 2_000).await;
    let expected_workspace = workspace.path().to_string_lossy().into_owned();
    assert_eq!(
        recorded,
        vec![(
            "idle".to_string(),
            "complete".to_string(),
            Some(expected_workspace)
        )],
        "SessionState(idle) hook input must carry the session record's workspace root"
    );
}
