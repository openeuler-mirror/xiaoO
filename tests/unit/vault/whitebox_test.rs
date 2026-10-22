use super::*;

#[test]
fn key_reconstruction_deterministic() {
    let provider = WhiteBoxKeyProvider::new("test");
    let key1 = provider.reconstruct_key().unwrap();
    let key2 = provider.reconstruct_key().unwrap();
    assert_eq!(key1, key2, "key reconstruction should be deterministic");
}

#[test]
fn differently_named_providers_yield_same_key() {
    let p1 = WhiteBoxKeyProvider::new("p1");
    let p2 = WhiteBoxKeyProvider::new("p2");
    let k1 = p1.reconstruct_key().unwrap();
    let k2 = p2.reconstruct_key().unwrap();
    assert_eq!(k1, k2, "same fragments should produce same key");
}

#[test]
fn reconstructed_key_is_32_bytes_for_aes_256() {
    let provider = WhiteBoxKeyProvider::new("test");
    let key = provider.reconstruct_key().unwrap();
    assert_eq!(key.len(), 32, "AES-256 requires 32 bytes");
}
