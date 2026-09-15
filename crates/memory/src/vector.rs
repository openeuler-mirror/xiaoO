use std::collections::HashMap;

/// Cosine similarity between two f32 vectors. Returns value in [0.0, 1.0].
/// Returns 0.0 for empty, mismatched, zero-norm, or NaN/Inf inputs.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }

    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return 0.0;
    }

    let sim = (dot / denom) as f32;
    if sim.is_nan() || sim.is_infinite() {
        return 0.0;
    }
    sim.clamp(0.0, 1.0)
}

/// Serialize f32 vector to little-endian bytes.
pub fn vec_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Deserialize little-endian bytes to f32 vector.
///
/// Returns empty vec if bytes is empty. Silently drops trailing bytes
/// that don't fill a complete f32 (< 4 bytes remainder) — this matches
/// SQLite BLOB storage semantics where the length is always exact.
pub fn bytes_to_vec(bytes: &[u8]) -> Vec<f32> {
    if !bytes.is_empty() && bytes.len() % 4 != 0 {
        // Log-worthy: indicates corrupted data. Return what we can parse.
        tracing::warn!(
            "embedding bytes length {} not divisible by 4, truncating remainder",
            bytes.len()
        );
    }
    bytes
        .chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = chunk.try_into().unwrap_or([0; 4]);
            f32::from_le_bytes(arr)
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct ScoredResult {
    pub id: String,
    pub vector_score: Option<f32>,
    pub keyword_score: Option<f32>,
    pub final_score: f32,
}

/// Weighted merge of vector and keyword search results.
///
/// Vector scores are assumed in [0, 1]. Keyword scores (BM25) are normalized
/// against max_keyword to [0, 1]. Results are deduplicated by id and combined
/// with `final_score = vector_weight * vs + keyword_weight * ks`.
pub fn hybrid_merge(
    vector_results: &[(String, f32)],
    keyword_results: &[(String, f32)],
    vector_weight: f32,
    keyword_weight: f32,
    limit: usize,
) -> Vec<ScoredResult> {
    if limit == 0 {
        return Vec::new();
    }

    let max_keyword = keyword_results
        .iter()
        .map(|(_, s)| *s)
        .fold(f32::NEG_INFINITY, f32::max);

    let mut map: HashMap<String, (Option<f32>, Option<f32>)> = HashMap::new();

    for (id, score) in vector_results {
        let entry = map.entry(id.clone()).or_insert((None, None));
        let prev = entry.0.unwrap_or(0.0);
        entry.0 = Some(prev.max(*score));
    }

    for (id, score) in keyword_results {
        let normalized = if max_keyword > 0.0 {
            score / max_keyword
        } else {
            0.0
        };
        let entry = map.entry(id.clone()).or_insert((None, None));
        let prev = entry.1.unwrap_or(0.0);
        entry.1 = Some(prev.max(normalized));
    }

    let mut results: Vec<ScoredResult> = map
        .into_iter()
        .map(|(id, (vs, ks))| {
            let final_score =
                vector_weight * vs.unwrap_or(0.0) + keyword_weight * ks.unwrap_or(0.0);
            ScoredResult {
                id,
                vector_score: vs,
                keyword_score: ks,
                final_score,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    results
}

#[cfg(test)]
#[path = "../../../tests/unit/memory/vector_test.rs"]
mod tests;
