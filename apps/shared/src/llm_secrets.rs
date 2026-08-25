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

pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()> {
    let secrets_path = llm_secrets_path(config_path);
    let secrets = load_llm_secrets(&secrets_path)?;
    for (key, value) in secrets {
        std::env::set_var(key, value);
    }
    Ok(())
}

fn load_llm_secrets(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read secrets file {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse secrets file {}", path.display()))
}

fn llm_secrets_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LLM_SECRETS_FILE)
}

#[cfg(test)]
mod tests {
    use super::{inject_llm_secrets_into_env, save_llm_secret};

    #[test]
    fn saved_secret_is_injected_from_config_directory() {
        let temp_dir = tempfile::TempDir::new().expect("create temp dir");
        let config_path = temp_dir.path().join("config.toml");
        let env_name = "XIAOO_TEST_DEEPSEEK_API_KEY";

        std::env::remove_var(env_name);
        save_llm_secret(&config_path, env_name, "test-secret").expect("save secret");
        inject_llm_secrets_into_env(&config_path).expect("inject secret");

        assert_eq!(std::env::var(env_name).as_deref(), Ok("test-secret"));
        std::env::remove_var(env_name);
    }
}
