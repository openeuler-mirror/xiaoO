use super::*;

#[test]
fn in_memory_store_supports_crud() {
    let store = InMemoryAuthStore::new();
    let cred = AuthCredential::new("test-1", "openai", "sk-test");
    store.save(std::slice::from_ref(&cred)).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "test-1");

    let updated = AuthCredential::new("test-1", "openai", "sk-updated");
    store.update(&updated).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded[0].api_key, "sk-updated");

    store.delete("test-1").unwrap();
    let loaded = store.load().unwrap();
    assert!(loaded.is_empty());
}

#[test]
fn in_memory_store_reports_not_found() {
    let store = InMemoryAuthStore::new();
    let result = store.update(&AuthCredential::new("nonexistent", "openai", "key"));
    assert!(matches!(result, Err(AuthStoreError::NotFound(_))));
    let result = store.delete("nonexistent");
    assert!(matches!(result, Err(AuthStoreError::NotFound(_))));
}
