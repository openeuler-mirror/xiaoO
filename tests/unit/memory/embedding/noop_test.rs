use super::*;

#[tokio::test]
async fn noop_returns_empty_vectors() {
    let noop = NoopEmbedding;
    assert_eq!(noop.name(), "noop");
    assert_eq!(noop.dimensions(), 0);

    let result = noop.embed(&["hello", "world"]).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result[0].is_empty());

    let single = noop.embed_one("test").await.unwrap();
    assert!(single.is_empty());
}
