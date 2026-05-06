pub const DEFAULT_TIMEOUT_ENV_VAR: &str = "BASH_DEFAULT_TIMEOUT_MS";
pub const MAX_TIMEOUT_ENV_VAR: &str = "BASH_MAX_TIMEOUT_MS";

pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;
pub const MAX_OUTPUT_BYTES_PER_STREAM: usize = 1024 * 1024;

fn read_positive_env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
}

pub fn default_timeout_ms() -> u64 {
    read_positive_env_u64(DEFAULT_TIMEOUT_ENV_VAR).unwrap_or(DEFAULT_TIMEOUT_MS)
}

pub fn max_timeout_ms() -> u64 {
    let default_timeout = default_timeout_ms();
    let configured_max = read_positive_env_u64(MAX_TIMEOUT_ENV_VAR).unwrap_or(MAX_TIMEOUT_MS);
    configured_max.max(default_timeout)
}
