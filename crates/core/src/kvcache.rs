use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KvCacheMap(HashMap<String, String>);

impl KvCacheMap {
    pub fn chunk_hashes(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }

    pub fn diff_deleted(&self, new_hashes: &[String]) -> Vec<String> {
        let new_set: HashSet<&String> = new_hashes.iter().collect();
        self.0
            .keys()
            .filter(|h| !new_set.contains(*h))
            .cloned()
            .collect()
    }

    pub fn replace(&mut self, new_hashes: &[String], text: &str) {
        self.0.clear();
        for h in new_hashes {
            self.0.insert(h.clone(), text.to_string());
        }
    }
}

pub fn spawn_prefetch(chunk_hashes: Vec<String>, lookup_id: String) {
    if chunk_hashes.is_empty() {
        return;
    }
    let count = chunk_hashes.len();
    tokio::spawn(async move {
        let payload = serde_json::json!({
            "chunk_hashes": chunk_hashes,
            "lookup_id": lookup_id,
        });
        match reqwest::Client::new()
            .post("http://localhost:6999/memory/prefetch")
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => {
                let status = resp.status();
                tracing::info!(
                    status = %status,
                    count = count,
                    "kv cache prefetch completed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    count = count,
                    "kv cache prefetch failed"
                );
            }
        }
    });
}

pub fn spawn_evict(deleted_hashes: Vec<String>) {
    if deleted_hashes.is_empty() {
        return;
    }
    let count = deleted_hashes.len();
    tokio::spawn(async move {
        let payload = serde_json::json!({
            "chunk_hashes": deleted_hashes,
        });
        match reqwest::Client::new()
            .post("http://localhost:6999/memory/evict")
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) => {
                tracing::info!(
                    status = %resp.status(),
                    count = count,
                    "kv cache evict completed"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    count = count,
                    "kv cache evict failed"
                );
            }
        }
    });
}
