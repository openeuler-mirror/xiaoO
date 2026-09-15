use crate::daemon_config::{DaemonConfig, LlmProfileConfig};
use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::time::Instant;
use xiaoo_shared::gateway::llm_assembly::{
    list_llm_models, probe_llm_provider, LlmAssemblyInput, LlmModelSummary,
};

#[derive(Debug, Serialize)]
pub struct ModelConnectionReport {
    pub schema_version: u32,
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    pub success: bool,
    pub latency_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelCatalogReport {
    pub schema_version: u32,
    pub profile_id: String,
    pub provider: String,
    pub success: bool,
    pub models: Vec<LlmModelSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub async fn test_model_connection(
    config_path: &Path,
    profile_id: &str,
) -> Result<ModelConnectionReport> {
    let config = DaemonConfig::load_from(config_path)?;
    let profile = configured_profile(&config, profile_id)?;
    let started = Instant::now();
    let result = probe_llm_provider(assembly_input(config_path, profile)).await;
    Ok(ModelConnectionReport {
        schema_version: 1,
        profile_id: profile_id.to_string(),
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        success: result.is_ok(),
        latency_ms: started.elapsed().as_millis(),
        error: result.err().map(|error| error.to_string()),
    })
}

pub async fn list_model_catalog(
    config_path: &Path,
    profile_id: &str,
) -> Result<ModelCatalogReport> {
    let config = DaemonConfig::load_from(config_path)?;
    let profile = configured_profile(&config, profile_id)?;
    let result = list_llm_models(assembly_input(config_path, profile)).await;
    let (models, error) = match result {
        Ok(models) => (models, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    Ok(ModelCatalogReport {
        schema_version: 1,
        profile_id: profile_id.to_string(),
        provider: profile.provider.clone(),
        success: error.is_none(),
        models,
        error,
    })
}

fn configured_profile<'a>(
    config: &'a DaemonConfig,
    profile_id: &str,
) -> Result<&'a LlmProfileConfig> {
    let profile_id = profile_id.trim();
    if profile_id.is_empty() {
        anyhow::bail!("model profile id must not be empty");
    }
    config
        .app
        .llm
        .profiles
        .get(profile_id)
        .with_context(|| format!("model profile `{profile_id}` does not exist"))
}

fn assembly_input(config_path: &Path, profile: &LlmProfileConfig) -> LlmAssemblyInput {
    let environment = profile.api_key_env.clone().or_else(|| {
        xiaoo_api::llm::resolve_provider_profile(profile.provider.trim())
            .and_then(|provider| provider.default_api_key_env.map(str::to_string))
    });
    let api_key = environment.as_deref().and_then(|environment| {
        xiaoo_shared::llm_secrets::get_llm_secret(config_path, environment).ok()
    });
    LlmAssemblyInput {
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        api_base: profile.api_base.clone(),
        api_key: api_key.clone(),
        api_key_env: api_key
            .is_none()
            .then(|| profile.api_key_env.clone())
            .flatten(),
        context_window_override: profile
            .context_window
            .and_then(|value| u32::try_from(value).ok()),
        agent_id: None,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/serverside/model_management_test.rs"]
mod tests;
