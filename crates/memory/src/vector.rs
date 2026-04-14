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
        eprintln!(
            "warning: embedding bytes length {} not divisible by 4, truncating remainder",
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
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_vectors() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn cosine_empty_returns_zero() {
        assert_eq!(cosine_similarity(&[], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn cosine_mismatched_length() {
        assert_eq!(cosine_similarity(&[1.0, 2.0], &[1.0]), 0.0);
    }

    #[test]
    fn cosine_zero_vector() {
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn vec_bytes_roundtrip() {
        let v = vec![1.0_f32, -2.5, 3.14, 0.0, f32::MAX];
        assert_eq!(bytes_to_vec(&vec_to_bytes(&v)), v);
    }

    #[test]
    fn vec_bytes_empty() {
        assert!(bytes_to_vec(&vec_to_bytes(&[])).is_empty());
    }

    #[test]
    fn vec_bytes_non_aligned_truncates() {
        // 5 bytes → only 1 f32 (4 bytes), last byte dropped
        let bytes = vec![0u8, 0, 128, 63, 99]; // 1.0f32 le + 1 extra byte
        let result = bytes_to_vec(&bytes);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_nan_returns_zero() {
        let a = vec![f32::NAN, 1.0];
        let b = vec![1.0, 1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn cosine_high_dimensional() {
        // 1536-dim like real embeddings
        let a: Vec<f32> = (0..1536).map(|i| (i as f32).sin()).collect();
        let b = a.clone();
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn hybrid_merge_vector_only() {
        let vector = vec![("a".into(), 0.9), ("b".into(), 0.5)];
        let merged = hybrid_merge(&vector, &[], 0.7, 0.3, 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a");
    }

    #[test]
    fn hybrid_merge_keyword_only() {
        let keyword = vec![("a".into(), 5.0), ("b".into(), 3.0)];
        let merged = hybrid_merge(&[], &keyword, 0.7, 0.3, 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "a");
    }

    #[test]
    fn hybrid_merge_deduplication() {
        let vector = vec![("a".into(), 0.8)];
        let keyword = vec![("a".into(), 5.0), ("b".into(), 3.0)];
        let merged = hybrid_merge(&vector, &keyword, 0.7, 0.3, 10);
        assert!(merged
            .iter()
            .any(|r| r.id == "a" && r.vector_score.is_some() && r.keyword_score.is_some()));
    }

    #[test]
    fn hybrid_merge_respects_limit() {
        let vector: Vec<_> = (0..20).map(|i| (format!("v{i}"), 0.5)).collect();
        let merged = hybrid_merge(&vector, &[], 1.0, 0.0, 3);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn hybrid_merge_limit_zero() {
        let merged = hybrid_merge(&[("a".into(), 1.0)], &[], 1.0, 0.0, 0);
        assert!(merged.is_empty());
    }
}
