use super::*;

#[test]
fn test_credential_creation() {
    let cred = AuthCredential::new("test-1", "openai", "sk-test").with_priority(10);
    assert_eq!(cred.id, "test-1");
    assert_eq!(cred.provider, "openai");
    assert_eq!(cred.priority, 10);
    assert!(cred.is_available());
}

#[test]
fn test_pool_add_and_get() {
    let pool = InMemoryAuthPool::new();
    let cred = AuthCredential::new("test-1", "openai", "sk-test");
    pool.add(cred);
    let retrieved = pool.get("test-1");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().provider, "openai");
}

#[test]
fn test_pool_get_available() {
    let pool = InMemoryAuthPool::new();
    pool.add(AuthCredential::new("cred-1", "openai", "key1").with_priority(10));
    pool.add(AuthCredential::new("cred-2", "openai", "key2").with_priority(5));
    pool.add(AuthCredential::new("cred-3", "anthropic", "key3").with_priority(1));

    let available = pool.get_available("openai", "gpt-4");
    assert!(available.is_some());
    assert_eq!(available.unwrap().id, "cred-2");

    let other = pool.get_available("gemini", "gemini-pro");
    assert!(other.is_none());
}

#[test]
fn test_pool_mark_cooldown() {
    let pool = InMemoryAuthPool::new();
    pool.add(AuthCredential::new("test-1", "openai", "sk-test"));
    pool.mark_cooldown("test-1", Duration::from_secs(60));
    let cred = pool.get("test-1").unwrap();
    assert!(cred.state.is_in_cooldown());
    assert!(!cred.is_available());
}

#[test]
fn test_pool_mark_blocked() {
    let pool = InMemoryAuthPool::new();
    pool.add(AuthCredential::new("test-1", "openai", "sk-test"));
    pool.mark_blocked("test-1", "rate limited");
    let cred = pool.get("test-1").unwrap();
    assert!(cred.state.is_blocked());
    assert_eq!(cred.state.block_reason(), Some("rate limited"));
}

#[test]
fn test_pool_remove() {
    let pool = InMemoryAuthPool::new();
    pool.add(AuthCredential::new("test-1", "openai", "sk-test"));
    pool.remove("test-1");
    assert!(pool.get("test-1").is_none());
}
