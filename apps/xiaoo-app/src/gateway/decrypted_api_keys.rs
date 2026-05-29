use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

static DECRYPTED_API_KEYS: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

pub fn get_decrypted_api_key(env_name: &str) -> Option<String> {
    DECRYPTED_API_KEYS
        .get()?
        .read()
        .ok()?
        .get(env_name)
        .cloned()
}

pub fn store_decrypted_api_keys(api_keys: HashMap<String, String>) {
    let rw_lock = DECRYPTED_API_KEYS.get_or_init(|| RwLock::new(HashMap::new()));
    if let Ok(mut guard) = rw_lock.write() {
        for (key, value) in api_keys {
            guard.insert(key, value);
        }
    }
}
