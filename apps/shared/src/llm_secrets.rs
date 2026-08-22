use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const LLM_SECRETS_FILE: &str = "llm_secrets.json";

pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()> {
    let secrets_path = llm_secrets_path(config_path);
    if let Some(parent) = secrets_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create secrets directory {}", parent.display()))?;
    }

    let mut secrets = load_llm_secrets(&secrets_path)?;
    secrets.insert(env_name.to_string(), secret.to_string());
    fs::write(
        &secrets_path,
        serde_json::to_vec_pretty(&secrets).context("failed to serialize llm secrets")?,
    )
    .with_context(|| format!("failed to write secrets file {}", secrets_path.display()))?;
    Ok(())
}

pub fn inject_llm_secrets_into_env(_config_path: &Path) -> Result<()> {
    Ok(())
}

fn load_llm_secrets(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("failed to read secrets file {}: {}", path.display(), e);
            return Ok(BTreeMap::new());
        }
    };
    match serde_json::from_slice::<BTreeMap<String, String>>(&bytes) {
        Ok(secrets) => Ok(secrets),
        Err(e) => {
            tracing::warn!(
                "failed to parse secrets file {}: {}; skipping secrets loading",
                path.display(),
                e
            );
            Ok(BTreeMap::new())
        }
    }
}

fn llm_secrets_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LLM_SECRETS_FILE)
}
