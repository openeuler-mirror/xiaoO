use std::borrow::Cow;
use std::sync::{Arc, Mutex, RwLock};

use agent_contracts::runtime::RuntimeView;
use agent_contracts::trace::{TraceOutcome, TraceSpanHandle, TraceSpanKind};
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::hook::HookPointId;
use agent_types::hook::{HookInvokeInput, HookInvokeMetadata, HookInvokeOutput};
use agent_types::llm::{
    ErrorLlmHookInput, ErrorLlmHookResult, LlmError, LlmRequest, LlmResponse, PostLlmHookInput,
    PostLlmHookResult, PreLlmHookInput, PreLlmHookResult, StreamChunk,
};
use async_trait::async_trait;
use hook::{resolve_hook_point_category, HookPointCategory};
use serde_json::json;

use super::trace::{
    begin_trace_span, effective_request_trace_fields, end_trace_span, error_trace_fields,
    llm_error_kind, merge_trace_fields, response_trace_fields, stream_trace_fields,
    trace_outcome_for_error, update_trace_span, StreamTraceStats,
};

pub struct LlmProviderWrapper {
    inner: Arc<dyn LlmProvider>,
    /// Present only when hooks are enabled.
    agent_id: Option<String>,
    runtime_view: RwLock<Option<Arc<dyn RuntimeView>>>,
}

impl LlmProviderWrapper {
    /// Constructs a wrapper around the given provider.  When `agent_id` and
    /// `runtime_view` are both `Some`, hooks fire on every `complete` /
    /// `complete_stream` call.  Pass `None` for either to disable hooks.
    pub fn new(
        inner: Arc<dyn LlmProvider>,
        agent_id: Option<String>,
        runtime_view: Option<Arc<dyn RuntimeView>>,
    ) -> Self {
        Self {
            inner,
            agent_id,
            runtime_view: RwLock::new(runtime_view),
        }
    }

    /// Injects a `RuntimeView` into this wrapper after construction, enabling
    /// hooks.  Intended to be called once the runtime view is available (e.g.
    /// after `AppRuntimeFactory::build`).
    pub fn set_runtime_view(&self, runtime_view: Arc<dyn RuntimeView>) {
        if let Ok(mut guard) = self.runtime_view.write() {
            *guard = Some(runtime_view);
        }
    }

    /// Returns the raw inner provider that this wrapper delegates to.
    pub fn inner(&self) -> Arc<dyn LlmProvider> {
        self.inner.clone()
    }

    fn build_llm_hook_point(&self, stage: &str) -> Option<HookPointId> {
        self.agent_id
            .as_deref()
            .map(|id| HookPointId(format!("{}.Llm.complete.{}", id, stage)))
    }

    fn runtime_view(&self) -> Option<Arc<dyn RuntimeView>> {
        let guard = self.runtime_view.read().unwrap();
        guard.as_ref().cloned()
    }

    async fn run_pre_hook_sequence(
        &self,
        request: &mut LlmRequest,
    ) -> Result<Vec<PreLlmHookResult>, LlmError> {
        let runtime_view = {
            let guard = self.runtime_view.read().unwrap();
            guard.as_ref().cloned()
        };
        let runtime_view = match runtime_view {
            Some(rv) => rv,
            None => return Ok(Vec::new()),
        };

        let hook_point = match self.build_llm_hook_point("pre") {
            Some(hp) => hp,
            None => return Ok(Vec::new()),
        };

        let category =
            resolve_hook_point_category(&hook_point).map_err(|e| LlmError::RequestFailed {
                message: format!(
                    "failed to resolve pre-hook category (hook_point='{}'): {}",
                    hook_point.0, e
                ),
            })?;

        if category != HookPointCategory::LlmPre {
            return Err(LlmError::RequestFailed {
                message: format!(
                    "pre-hook sequence expected LlmPre category but got {:?} for hook point {}",
                    category, hook_point.0
                ),
            });
        }

        let registry = runtime_view.hookers();
        let mut hookers = registry.list_for_hook_point(&hook_point);
        hookers.retain(|h| registry.is_enabled(h.id()));
        hookers.sort_by(|a, b| a.id().0.cmp(&b.id().0));

        if hookers.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for hooker in hookers {
            let hook_span = runtime_view
                .trace_recorder()
                .begin_span(
                    TraceSpanKind::Hook,
                    Cow::Borrowed("llm_pre_hook"),
                    json!({
                        "hook_kind": "llm_pre",
                        "hooker_id": hooker.id().to_string(),
                        "hook_point": hook_point.0,
                    }),
                )
                .await;

            let input = HookInvokeInput::LlmPre {
                input: PreLlmHookInput {
                    request: request.clone(),
                },
                metadata: hook_invoke_metadata(&hook_span),
            };

            let output = match hooker.invoke(input, runtime_view.as_ref()).await {
                Ok(o) => o,
                Err(e) => {
                    eprintln!(
                        "llm pre-hook invoke failed for hooker '{}' (hook_point='{}'): {}",
                        hooker.id(),
                        hook_point.0,
                        e
                    );
                    runtime_view
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": e.to_string()}),
                        )
                        .await;
                    continue;
                }
            };

            let pre_result = match output {
                HookInvokeOutput::LlmPre(r) => r,
                other => {
                    eprintln!(
                        "llm pre-hooker '{}' returned unexpected output {:?} for hook_point '{}'",
                        hooker.id(),
                        other,
                        hook_point.0
                    );
                    runtime_view
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": "unexpected output variant"}),
                        )
                        .await;
                    continue;
                }
            };

            match pre_result {
                PreLlmHookResult::Allow => {
                    results.push(PreLlmHookResult::Allow);
                    runtime_view
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "allow"}))
                        .await;
                }
                PreLlmHookResult::Transform {
                    ref modified_request,
                } => {
                    *request = modified_request.clone();
                    runtime_view
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "transform"}))
                        .await;
                    results.push(pre_result);
                }
            }
        }

        Ok(results)
    }

    async fn run_post_hook_sequence(
        &self,
        request: &LlmRequest,
        response: &mut LlmResponse,
    ) -> Result<Vec<PostLlmHookResult>, LlmError> {
        let runtime_view = {
            let guard = self.runtime_view.read().unwrap();
            guard.as_ref().cloned()
        };
        let runtime_view = match runtime_view {
            Some(rv) => rv,
            None => return Ok(Vec::new()),
        };

        let hook_point = match self.build_llm_hook_point("post") {
            Some(hp) => hp,
            None => return Ok(Vec::new()),
        };

        let category =
            resolve_hook_point_category(&hook_point).map_err(|e| LlmError::RequestFailed {
                message: format!(
                    "failed to resolve post-hook category (hook_point='{}'): {}",
                    hook_point.0, e
                ),
            })?;

        if category != HookPointCategory::LlmPost {
            return Err(LlmError::RequestFailed {
                message: format!(
                    "post-hook sequence expected LlmPost category but got {:?} for hook point {}",
                    category, hook_point.0
                ),
            });
        }

        let registry = runtime_view.hookers();
        let mut hookers = registry.list_for_hook_point(&hook_point);
        hookers.retain(|h| registry.is_enabled(h.id()));
        hookers.sort_by(|a, b| a.id().0.cmp(&b.id().0));

        if hookers.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for hooker in hookers {
            let hook_span = runtime_view
                .trace_recorder()
                .begin_span(
                    TraceSpanKind::Hook,
                    Cow::Borrowed("llm_post_hook"),
                    json!({
                        "hook_kind": "llm_post",
                        "hooker_id": hooker.id().to_string(),
                        "hook_point": hook_point.0,
                    }),
                )
                .await;

            let input = HookInvokeInput::LlmPost {
                input: PostLlmHookInput {
                    request: request.clone(),
                    response: response.clone(),
                },
                metadata: hook_invoke_metadata(&hook_span),
            };

            let output = match hooker
                .invoke(input, runtime_view.as_ref())
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!(
                        "llm post-hook invoke failed for hooker '{}' (hook_point='{}'): {}",
                        hooker.id(),
                        hook_point.0,
                        e
                    ),
                }) {
                Ok(o) => o,
                Err(e) => {
                    runtime_view
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": e.to_string()}),
                        )
                        .await;
                    return Err(e);
                }
            };

            let post_result = match output {
                HookInvokeOutput::LlmPost(r) => r,
                other => {
                    let err = LlmError::RequestFailed {
                        message: format!(
                            "llm post-hooker '{}' returned unexpected output {:?} for hook_point '{}'",
                            hooker.id(),
                            other,
                            hook_point.0
                        ),
                    };
                    runtime_view
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": err.to_string()}),
                        )
                        .await;
                    return Err(err);
                }
            };

            match post_result {
                PostLlmHookResult::Accept => {
                    results.push(PostLlmHookResult::Accept);
                    runtime_view
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "accept"}))
                        .await;
                }
                PostLlmHookResult::Transform {
                    ref modified_response,
                } => {
                    *response = modified_response.clone();
                    runtime_view
                        .trace_recorder()
                        .end_span(hook_span, TraceOutcome::Ok, json!({"result": "transform"}))
                        .await;
                    results.push(post_result);
                }
            }
        }

        Ok(results)
    }

    async fn run_error_hook_sequence(
        &self,
        request: &LlmRequest,
        error: &LlmError,
    ) -> Result<Vec<ErrorLlmHookResult>, LlmError> {
        let runtime_view = {
            let guard = self.runtime_view.read().unwrap();
            guard.as_ref().cloned()
        };
        let runtime_view = match runtime_view {
            Some(rv) => rv,
            None => return Ok(Vec::new()),
        };

        let hook_point = match self.build_llm_hook_point("error") {
            Some(hp) => hp,
            None => return Ok(Vec::new()),
        };

        let category =
            resolve_hook_point_category(&hook_point).map_err(|e| LlmError::RequestFailed {
                message: format!(
                    "failed to resolve error-hook category (hook_point='{}'): {}",
                    hook_point.0, e
                ),
            })?;

        if category != HookPointCategory::LlmError {
            return Err(LlmError::RequestFailed {
                message: format!(
                    "error-hook sequence expected LlmError category but got {:?} for hook point {}",
                    category, hook_point.0
                ),
            });
        }

        let registry = runtime_view.hookers();
        let mut hookers = registry.list_for_hook_point(&hook_point);
        hookers.retain(|h| registry.is_enabled(h.id()));
        hookers.sort_by(|a, b| a.id().0.cmp(&b.id().0));

        if hookers.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for hooker in hookers {
            let hook_span = runtime_view
                .trace_recorder()
                .begin_span(
                    TraceSpanKind::Hook,
                    Cow::Borrowed("llm_error_hook"),
                    json!({
                        "hook_kind": "llm_error",
                        "hooker_id": hooker.id().to_string(),
                        "hook_point": hook_point.0,
                        "error": error.to_string(),
                    }),
                )
                .await;

            let input = HookInvokeInput::LlmError {
                input: ErrorLlmHookInput {
                    request: request.clone(),
                    error: error.clone(),
                },
                metadata: hook_invoke_metadata(&hook_span),
            };

            let output = match hooker
                .invoke(input, runtime_view.as_ref())
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!(
                        "llm error-hook invoke failed for hooker '{}' (hook_point='{}'): {}",
                        hooker.id(),
                        hook_point.0,
                        e
                    ),
                }) {
                Ok(o) => o,
                Err(e) => {
                    runtime_view
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": e.to_string()}),
                        )
                        .await;
                    return Err(e);
                }
            };

            let error_result = match output {
                HookInvokeOutput::LlmError(r) => r,
                other => {
                    let err = LlmError::RequestFailed {
                        message: format!(
                            "llm error-hooker '{}' returned unexpected output {:?} for hook_point '{}'",
                            hooker.id(),
                            other,
                            hook_point.0
                        ),
                    };
                    runtime_view
                        .trace_recorder()
                        .end_span(
                            hook_span,
                            TraceOutcome::Error,
                            json!({"error": err.to_string()}),
                        )
                        .await;
                    return Err(err);
                }
            };

            let (trace_outcome, result_label) = match &error_result {
                ErrorLlmHookResult::Propagate => (TraceOutcome::Ok, "propagate"),
                ErrorLlmHookResult::Recover { .. } => (TraceOutcome::Ok, "recover"),
            };
            runtime_view
                .trace_recorder()
                .end_span(hook_span, trace_outcome, json!({"result": result_label}))
                .await;

            results.push(error_result);
        }

        Ok(results)
    }
}

fn hook_invoke_metadata(hook_span: &TraceSpanHandle) -> HookInvokeMetadata {
    HookInvokeMetadata {
        trace_id: Some(hook_span.trace_id().to_string()),
        span_id: Some(hook_span.span_id().to_string()),
        parent_span_id: hook_span.parent_span_id().map(ToString::to_string),
    }
}

impl LlmProviderWrapper {
    pub async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut effective_request = request.clone();
        let runtime_view = self.runtime_view();
        let runtime_ref = runtime_view.as_deref();
        let mut trace_span = begin_trace_span(
            runtime_ref,
            &self.capabilities().model_name,
            &effective_request,
            false,
        )
        .await;
        let mut pre_hook_count = 0;
        let mut post_hook_count = 0;
        let mut error_hook_count = 0;
        let mut pre_hook_error = None;
        let mut post_hook_error = None;
        let mut error_hook_error = None;

        if runtime_view.is_some() {
            match self.run_pre_hook_sequence(&mut effective_request).await {
                Ok(results) => {
                    pre_hook_count = results.len();
                }
                Err(e) => {
                    pre_hook_error = Some(e.to_string());
                    eprintln!("llm pre-hook phase failed: {}", e);
                }
            }

            update_trace_span(
                runtime_ref,
                trace_span.as_ref(),
                merge_trace_fields(
                    json!({
                        "phase": "pre_hooks_done",
                        "pre_hook_count": pre_hook_count,
                        "pre_hook_error": pre_hook_error,
                    }),
                    effective_request_trace_fields(&effective_request),
                ),
            )
            .await;
        }

        match self.inner.complete(&effective_request).await {
            Ok(mut response) => {
                update_trace_span(
                    runtime_ref,
                    trace_span.as_ref(),
                    json!({
                        "phase": "provider_completed",
                    }),
                )
                .await;

                if runtime_view.is_some() {
                    match self
                        .run_post_hook_sequence(&effective_request, &mut response)
                        .await
                    {
                        Ok(results) => {
                            post_hook_count = results.len();
                        }
                        Err(e) => {
                            post_hook_error = Some(e.to_string());
                            eprintln!("llm post-hook phase failed: {}", e);
                        }
                    }
                }

                end_trace_span(
                    runtime_ref,
                    &mut trace_span,
                    agent_contracts::TraceOutcome::Ok,
                    merge_trace_fields(
                        json!({
                            "phase": "finished",
                            "pre_hook_count": pre_hook_count,
                            "post_hook_count": post_hook_count,
                            "error_hook_count": error_hook_count,
                            "pre_hook_error": pre_hook_error,
                            "post_hook_error": post_hook_error,
                            "error_hook_error": error_hook_error,
                            "recovered": false,
                        }),
                        response_trace_fields(&response),
                    ),
                )
                .await;
                Ok(response)
            }
            Err(error) => {
                if runtime_view.is_some() {
                    match self
                        .run_error_hook_sequence(&effective_request, &error)
                        .await
                    {
                        Ok(results) => {
                            error_hook_count = results.len();
                            for result in results {
                                if let ErrorLlmHookResult::Recover { response } = result {
                                    end_trace_span(
                                        runtime_ref,
                                        &mut trace_span,
                                        agent_contracts::TraceOutcome::Ok,
                                        merge_trace_fields(
                                            json!({
                                                "phase": "finished",
                                                "pre_hook_count": pre_hook_count,
                                                "post_hook_count": post_hook_count,
                                                "error_hook_count": error_hook_count,
                                                "pre_hook_error": pre_hook_error,
                                                "post_hook_error": post_hook_error,
                                                "error_hook_error": error_hook_error,
                                                "recovered": true,
                                                "original_error_kind": llm_error_kind(&error),
                                                "original_error_message": error.to_string(),
                                            }),
                                            response_trace_fields(&response),
                                        ),
                                    )
                                    .await;
                                    return Ok(response);
                                }
                            }
                        }
                        Err(e) => {
                            error_hook_error = Some(e.to_string());
                            eprintln!("llm error-hook phase failed: {}", e);
                        }
                    }
                }

                end_trace_span(
                    runtime_ref,
                    &mut trace_span,
                    trace_outcome_for_error(&error),
                    merge_trace_fields(
                        json!({
                            "phase": "finished",
                            "pre_hook_count": pre_hook_count,
                            "post_hook_count": post_hook_count,
                            "error_hook_count": error_hook_count,
                            "pre_hook_error": pre_hook_error,
                            "post_hook_error": post_hook_error,
                            "error_hook_error": error_hook_error,
                            "recovered": false,
                        }),
                        error_trace_fields(&error),
                    ),
                )
                .await;
                Err(error)
            }
        }
    }

    pub async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        let mut effective_request = request.clone();
        let runtime_view = self.runtime_view();
        let runtime_ref = runtime_view.as_deref();
        let mut trace_span = begin_trace_span(
            runtime_ref,
            &self.capabilities().model_name,
            &effective_request,
            true,
        )
        .await;
        let mut pre_hook_count = 0;
        let mut post_hook_count = 0;
        let mut error_hook_count = 0;
        let mut pre_hook_error = None;
        let mut post_hook_error = None;
        let mut error_hook_error = None;
        let stream_stats = Mutex::new(StreamTraceStats::default());
        let traced_on_chunk = |chunk: StreamChunk| {
            if let Ok(mut stats) = stream_stats.lock() {
                stats.chunk_count += 1;
                if let Some(text) = chunk.delta_text.as_ref() {
                    stats.text_bytes += text.len();
                }
                if chunk.delta_tool_call.is_some() {
                    stats.saw_tool_call_delta = true;
                }
            }
            on_chunk(chunk);
        };

        if runtime_view.is_some() {
            match self.run_pre_hook_sequence(&mut effective_request).await {
                Ok(results) => {
                    pre_hook_count = results.len();
                }
                Err(e) => {
                    pre_hook_error = Some(e.to_string());
                    eprintln!("llm pre-hook phase failed (stream): {}", e);
                }
            }

            update_trace_span(
                runtime_ref,
                trace_span.as_ref(),
                merge_trace_fields(
                    json!({
                        "phase": "pre_hooks_done",
                        "pre_hook_count": pre_hook_count,
                        "pre_hook_error": pre_hook_error,
                    }),
                    effective_request_trace_fields(&effective_request),
                ),
            )
            .await;
        }

        match self
            .inner
            .complete_stream(&effective_request, &traced_on_chunk)
            .await
        {
            Ok(mut response) => {
                update_trace_span(
                    runtime_ref,
                    trace_span.as_ref(),
                    json!({
                        "phase": "provider_completed",
                    }),
                )
                .await;

                if runtime_view.is_some() {
                    match self
                        .run_post_hook_sequence(&effective_request, &mut response)
                        .await
                    {
                        Ok(results) => {
                            post_hook_count = results.len();
                        }
                        Err(e) => {
                            post_hook_error = Some(e.to_string());
                            eprintln!("llm post-hook phase failed (stream): {}", e);
                        }
                    }
                }

                let stream_trace_fields = match stream_stats.into_inner() {
                    Ok(stats) => stream_trace_fields(&stats),
                    Err(poisoned) => {
                        let stats: StreamTraceStats = poisoned.into_inner();
                        stream_trace_fields(&stats)
                    }
                };

                end_trace_span(
                    runtime_ref,
                    &mut trace_span,
                    agent_contracts::TraceOutcome::Ok,
                    merge_trace_fields(
                        merge_trace_fields(
                            json!({
                                "phase": "finished",
                                "pre_hook_count": pre_hook_count,
                                "post_hook_count": post_hook_count,
                                "error_hook_count": error_hook_count,
                                "pre_hook_error": pre_hook_error,
                                "post_hook_error": post_hook_error,
                                "error_hook_error": error_hook_error,
                                "recovered": false,
                            }),
                            response_trace_fields(&response),
                        ),
                        stream_trace_fields,
                    ),
                )
                .await;
                Ok(response)
            }
            Err(error) => {
                if runtime_view.is_some() {
                    match self
                        .run_error_hook_sequence(&effective_request, &error)
                        .await
                    {
                        Ok(results) => {
                            error_hook_count = results.len();
                            for result in results {
                                if let ErrorLlmHookResult::Recover { response } = result {
                                    let stream_trace_fields = match stream_stats.into_inner() {
                                        Ok(stats) => stream_trace_fields(&stats),
                                        Err(poisoned) => {
                                            stream_trace_fields(&poisoned.into_inner())
                                        }
                                    };

                                    end_trace_span(
                                        runtime_ref,
                                        &mut trace_span,
                                        agent_contracts::TraceOutcome::Ok,
                                        merge_trace_fields(
                                            merge_trace_fields(
                                                json!({
                                                    "phase": "finished",
                                                    "pre_hook_count": pre_hook_count,
                                                    "post_hook_count": post_hook_count,
                                                    "error_hook_count": error_hook_count,
                                                    "pre_hook_error": pre_hook_error,
                                                    "post_hook_error": post_hook_error,
                                                    "error_hook_error": error_hook_error,
                                                    "recovered": true,
                                                    "original_error_kind": llm_error_kind(&error),
                                                    "original_error_message": error.to_string(),
                                                }),
                                                response_trace_fields(&response),
                                            ),
                                            stream_trace_fields,
                                        ),
                                    )
                                    .await;
                                    return Ok(response);
                                }
                            }
                        }
                        Err(e) => {
                            error_hook_error = Some(e.to_string());
                            eprintln!("llm error-hook phase failed (stream): {}", e);
                        }
                    }
                }

                let stream_trace_fields = match stream_stats.into_inner() {
                    Ok(stats) => stream_trace_fields(&stats),
                    Err(poisoned) => {
                        let stats: StreamTraceStats = poisoned.into_inner();
                        stream_trace_fields(&stats)
                    }
                };

                end_trace_span(
                    runtime_ref,
                    &mut trace_span,
                    trace_outcome_for_error(&error),
                    merge_trace_fields(
                        merge_trace_fields(
                            json!({
                                "phase": "finished",
                                "pre_hook_count": pre_hook_count,
                                "post_hook_count": post_hook_count,
                                "error_hook_count": error_hook_count,
                                "pre_hook_error": pre_hook_error,
                                "post_hook_error": post_hook_error,
                                "error_hook_error": error_hook_error,
                                "recovered": false,
                            }),
                            error_trace_fields(&error),
                        ),
                        stream_trace_fields,
                    ),
                )
                .await;
                Err(error)
            }
        }
    }

    pub fn capabilities(&self) -> &ProviderCapabilities {
        self.inner.capabilities()
    }
}

#[async_trait]
impl LlmProvider for LlmProviderWrapper {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        LlmProviderWrapper::complete(self, request).await
    }

    async fn complete_stream(
        &self,
        request: &LlmRequest,
        on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        LlmProviderWrapper::complete_stream(self, request, on_chunk).await
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        LlmProviderWrapper::capabilities(self)
    }
}
