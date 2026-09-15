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
    let v = vec![1.0_f32, -2.5, std::f32::consts::PI, 0.0, f32::MAX];
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
