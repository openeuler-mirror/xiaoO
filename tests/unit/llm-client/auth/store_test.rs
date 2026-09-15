use super::*;

#[test]
fn test_in_memory_store_crud() {
    let store = InMemoryAuthStore::new();
    let cred = AuthCredential::new("test-1", "openai", "sk-test");
    store.save(&[cred.clone()]).unwrap();

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
fn test_in_memory_store_not_found() {
    let store = InMemoryAuthStore::new();
    let result = store.update(&AuthCredential::new("nonexistent", "openai", "key"));
    assert!(matches!(result, Err(AuthStoreError::NotFound(_))));
    let result = store.delete("nonexistent");
    assert!(matches!(result, Err(AuthStoreError::NotFound(_))));
}

#[test]
fn test_file_auth_store_stub() {
    let store = FileAuthStore::new("/tmp/test.json");
    assert!(store.load().is_err());
    assert!(store.save(&[]).is_err());
}
