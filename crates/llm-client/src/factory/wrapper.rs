use std::sync::{Arc, RwLock};

use agent_contracts::runtime::RuntimeView;
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::hooker::HookPointId;
use agent_types::hooker::{HookInvokeInput, HookInvokeOutput};
use agent_types::llm::{
    ErrorLlmHookInput, ErrorLlmHookResult, LlmError, LlmRequest, LlmResponse, PostLlmHookInput,
    PostLlmHookResult, PreLlmHookInput, PreLlmHookResult, StreamChunk,
};
use async_trait::async_trait;
use hooker::{resolve_hook_point_category, HookPointCategory};

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
            let input = HookInvokeInput::LlmPre(PreLlmHookInput {
                request: request.clone(),
            });

            let output = match hooker.invoke(input, runtime_view.as_ref()).await {
                Ok(o) => o,
                Err(e) => {
                    eprintln!(
                        "llm pre-hook invoke failed for hooker '{}' (hook_point='{}'): {}",
                        hooker.id(),
                        hook_point.0,
                        e
                    );
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
                    continue;
                }
            };

            match pre_result {
                PreLlmHookResult::Allow => {
                    results.push(PreLlmHookResult::Allow);
                }
                PreLlmHookResult::Transform {
                    ref modified_request,
                } => {
                    *request = modified_request.clone();
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
            let input = HookInvokeInput::LlmPost(PostLlmHookInput {
                request: request.clone(),
                response: response.clone(),
            });

            let output = hooker
                .invoke(input, runtime_view.as_ref())
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!(
                        "llm post-hook invoke failed for hooker '{}' (hook_point='{}'): {}",
                        hooker.id(),
                        hook_point.0,
                        e
                    ),
                })?;

            let post_result = match output {
                HookInvokeOutput::LlmPost(r) => r,
                other => {
                    return Err(LlmError::RequestFailed {
                        message: format!(
                            "llm post-hooker '{}' returned unexpected output {:?} for hook_point '{}'",
                            hooker.id(),
                            other,
                            hook_point.0
                        ),
                    });
                }
            };

            match post_result {
                PostLlmHookResult::Accept => {
                    results.push(PostLlmHookResult::Accept);
                }
                PostLlmHookResult::Transform {
                    ref modified_response,
                } => {
                    *response = modified_response.clone();
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
            let input = HookInvokeInput::LlmError(ErrorLlmHookInput {
                request: request.clone(),
                error: error.clone(),
            });

            let output = hooker
                .invoke(input, runtime_view.as_ref())
                .await
                .map_err(|e| LlmError::RequestFailed {
                    message: format!(
                        "llm error-hook invoke failed for hooker '{}' (hook_point='{}'): {}",
                        hooker.id(),
                        hook_point.0,
                        e
                    ),
                })?;

            let error_result = match output {
                HookInvokeOutput::LlmError(r) => r,
                other => {
                    return Err(LlmError::RequestFailed {
                        message: format!(
                            "llm error-hooker '{}' returned unexpected output {:?} for hook_point '{}'",
                            hooker.id(),
                            other,
                            hook_point.0
                        ),
                    });
                }
            };

            results.push(error_result);
        }

        Ok(results)
    }
}

impl LlmProviderWrapper {
    pub async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        let mut effective_request = request.clone();

        if self.runtime_view.read().unwrap().is_some() {
            match self.run_pre_hook_sequence(&mut effective_request).await {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("llm pre-hook phase failed: {}", e);
                }
            }
        }

        match self.inner.complete(&effective_request).await {
            Ok(mut response) => {
                if self.runtime_view.read().unwrap().is_some() {
                    match self
                        .run_post_hook_sequence(&effective_request, &mut response)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("llm post-hook phase failed: {}", e);
                        }
                    }
                }
                Ok(response)
            }
            Err(error) => {
                if self.runtime_view.read().unwrap().is_some() {
                    match self
                        .run_error_hook_sequence(&effective_request, &error)
                        .await
                    {
                        Ok(results) => {
                            for result in results {
                                if let ErrorLlmHookResult::Recover { response } = result {
                                    return Ok(response);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("llm error-hook phase failed: {}", e);
                        }
                    }
                }
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

        if self.runtime_view.read().unwrap().is_some() {
            match self.run_pre_hook_sequence(&mut effective_request).await {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("llm pre-hook phase failed (stream): {}", e);
                }
            }
        }

        match self
            .inner
            .complete_stream(&effective_request, on_chunk)
            .await
        {
            Ok(mut response) => {
                if self.runtime_view.read().unwrap().is_some() {
                    match self
                        .run_post_hook_sequence(&effective_request, &mut response)
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            eprintln!("llm post-hook phase failed (stream): {}", e);
                        }
                    }
                }
                Ok(response)
            }
            Err(error) => {
                if self.runtime_view.read().unwrap().is_some() {
                    match self
                        .run_error_hook_sequence(&effective_request, &error)
                        .await
                    {
                        Ok(results) => {
                            for result in results {
                                if let ErrorLlmHookResult::Recover { response } = result {
                                    return Ok(response);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("llm error-hook phase failed (stream): {}", e);
                        }
                    }
                }
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
