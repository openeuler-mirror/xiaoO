//! Environment variable filtering for execution.

use crate::error::CerberusError;
use crate::filters::{EnvFilterConfig, ExecutionControl};
use crate::policy::Policy;
use crate::request::ExecRequest;

pub(super) fn apply_env_filtering(
    mut request: ExecRequest,
    policy: &Policy,
) -> Result<ExecRequest, CerberusError> {
    let mut config = EnvFilterConfig::new()
        .allowlist_mode(true)
        .on_deny(crate::filters::ViolationAction::Warn);

    for var in &policy.environment.whitelist {
        config = config.allow(var);
    }

    let mut merged_env: std::collections::HashMap<String, String> = std::env::vars().collect();
    merged_env.extend(request.env.iter().cloned());
    request.env = merged_env.into_iter().collect();

    let filtered = ExecutionControl::builder(request)
        .env_filter(config)
        .build()
        .apply_all()?;

    Ok(filtered.request)
}
