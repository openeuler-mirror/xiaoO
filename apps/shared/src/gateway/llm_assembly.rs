//! LLM provider 装配门面。
//!
//! serverside 的 provider 装配链（解析配置 + api-key 回退 + 建 provider +
//! 定上下文窗口）下沉为一个入口 [`build_llm_provider`]，与 shared resolver
//! 内已有调用（`hosted_runtime_resolver`）合并。
//!
//! 两条硬约束：
//! - 入参出参不出现底层细粒度类型：`ResolveInput` / `ResolvedConfig` /
//!   `create_llm_provider_from_resolved` / `resolve_provider_profile` /
//!   `resolve_model_context_length` 一律在本模块内部使用，不暴露。
//! - 新增 pub 项必须有应用消费者：serverside `daemon_runtime` 启动装配与
//!   resolver 缓存未命中路径都改调它，`llm-client` 随之从 serverside 消失。
//!
//! 返回类型用 `xiaoo_api::llm` 既有导出（`LlmProviderWrapper`）。

use std::env;
use std::sync::Arc;

use llm_client::{create_llm_provider_from_resolved, resolve_provider_profile, LlmProviderWrapper};
use xiaoo_api::chat::AgentId;
use xiaoo_api::llm::{resolve_config, resolve_model_context_length, ResolveInput};

use crate::gateway::get_decrypted_api_key;

/// LLM provider 装配输入。字段只用基本类型与 shared 已导出的句柄。
#[derive(Clone, Debug)]
pub struct LlmAssemblyInput {
    pub provider: String,
    pub model: String,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    /// 显式上下文窗口覆盖；`None` 或 `0` 触发动态探测 + 静态回退。
    pub context_window_override: Option<u32>,
    pub agent_id: Option<AgentId>,
}

/// LLM provider 装配错误。
#[derive(Debug, thiserror::Error)]
pub enum LlmAssemblyError {
    #[error("failed to resolve llm provider config: {0}")]
    Resolve(String),
    #[error("failed to create llm provider: {0}")]
    Create(String),
    #[error("missing required API key environment variable: {0}")]
    MissingApiKeyEnv(String),
    #[error("API key environment variable is not valid unicode: {0}")]
    InvalidApiKeyEnv(String),
}

/// 解析 api-key：显式 > 解密存储 > 环境变量 > provider profile 默认 env。
///
/// 与原 serverside `resolve_llm_api_key` / `resolve_api_key_env` 等价：
/// - `api_key` 直接给 → 用它。
/// - `api_key_env` 给了 → 先查 shared 解密存储，再查进程环境；缺失则报错
///   （应用装配时 api_key_env 是强约束）。
/// - 都没给 → 回退到 provider profile 的 `default_api_key_env`（如有），此时
///   缺失不报错（profile 可能允许匿名）。
fn resolve_api_key(input: &LlmAssemblyInput) -> Result<Option<String>, LlmAssemblyError> {
    if let Some(api_key) = input.api_key.as_ref() {
        return Ok(Some(api_key.clone()));
    }

    if let Some(env_name) = input.api_key_env.as_deref() {
        return resolve_api_key_env(env_name, true);
    }

    if let Some(env_name) = resolve_provider_profile(&input.provider)
        .and_then(|profile| profile.default_api_key_env.map(str::to_string))
    {
        return resolve_api_key_env(&env_name, false);
    }

    Ok(None)
}

fn resolve_api_key_env(
    env_name: &str,
    fail_when_missing: bool,
) -> Result<Option<String>, LlmAssemblyError> {
    if let Some(api_key) = get_decrypted_api_key(env_name) {
        if !api_key.trim().is_empty() {
            return Ok(Some(api_key));
        }
    }

    match env::var(env_name) {
        Ok(value) if !value.trim().is_empty() => Ok(Some(value)),
        Ok(_) | Err(env::VarError::NotPresent) if fail_when_missing => {
            Err(LlmAssemblyError::MissingApiKeyEnv(env_name.to_string()))
        }
        Ok(_) | Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(LlmAssemblyError::InvalidApiKeyEnv(env_name.to_string()))
        }
    }
}

/// 解析配置 → 建 provider → 定上下文窗口，一步到位。
///
/// serverside 两处调用点（启动装配、resolver 缓存未命中路径）都改调它。
/// 返回 `(provider, effective_context_window)`：上下文窗口
/// 优先级与原 `resolve_effective_context_window` 一致——显式覆盖 > 动态探测
/// > provider capabilities 静态回退（≥ 1）。
pub async fn build_llm_provider(
    input: LlmAssemblyInput,
) -> Result<(Arc<LlmProviderWrapper>, u32), LlmAssemblyError> {
    let api_key = resolve_api_key(&input)?;

    let resolved = resolve_config(ResolveInput {
        provider: Some(input.provider.clone()),
        protocol: None,
        api_key,
        api_key_env: None,
        base_url: input.api_base.clone(),
    })
    .map_err(|e| LlmAssemblyError::Resolve(e.to_string()))?;

    let agent_id = input.agent_id.as_ref().map(|id| id.0.clone());
    let created = create_llm_provider_from_resolved(&resolved, input.model.clone(), agent_id, None)
        .map_err(|e| LlmAssemblyError::Create(e.to_string()))?;

    let provider = Arc::new(created);
    let static_fallback = provider.capabilities().max_context_window;

    let effective_context_window = resolve_effective_context_window(
        &resolved,
        &input.model,
        input.context_window_override,
        static_fallback,
    )
    .await;

    // usize → u32：上下文窗口理论可达 u64，但实际模型均远小于 u32::MAX；
    // 越界时饱和到 u32::MAX 而非截断，避免误导调用方。
    let context_window = u32::try_from(effective_context_window).unwrap_or(u32::MAX);

    Ok((provider, context_window))
}

/// 与原 serverside `resolve_effective_context_window` 等价的内部实现。
async fn resolve_effective_context_window(
    resolved: &llm_client::ResolvedConfig,
    model: &str,
    configured: Option<u32>,
    static_fallback: usize,
) -> usize {
    if let Some(context_window) = configured.filter(|value| *value > 0) {
        return context_window as usize;
    }

    match resolve_model_context_length(resolved, model).await {
        Ok(Some(context_window)) => match usize::try_from(context_window) {
            Ok(value) if value > 0 => return value,
            Ok(_) => {}
            Err(_) => {}
        },
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                model = %model,
                error = %error,
                "failed to dynamically resolve model context window; falling back"
            );
        }
    }

    static_fallback.max(1)
}
