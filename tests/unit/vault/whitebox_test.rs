use super::*;

#[test]
fn legacy_v1_master_key_is_deterministic_and_32_bytes() {
    let key = legacy_v1_master_key().expect("derive legacy key");
    assert_eq!(key.len(), 32);
    assert_eq!(
        key,
        legacy_v1_master_key().expect("derive legacy key again")
    );
}
