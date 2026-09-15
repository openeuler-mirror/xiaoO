use super::*;

#[test]
fn test_ready_state_is_available() {
    let state = AuthState::ready();
    assert!(state.is_available());
    assert!(!state.is_in_cooldown());
    assert!(!state.is_blocked());
    assert!(!state.is_disabled());
}

#[test]
fn test_cooldown_state_not_available() {
    let state = AuthState::cooldown(std::time::Duration::from_secs(60));
    assert!(!state.is_available());
    assert!(state.is_in_cooldown());
    assert!(state.cooldown_remaining().is_some());
}

#[test]
fn test_blocked_state_not_available() {
    let state = AuthState::blocked("test error");
    assert!(!state.is_available());
    assert!(state.is_blocked());
    assert_eq!(state.block_reason(), Some("test error"));
}

#[test]
fn test_disabled_state_not_available() {
    let state = AuthState::disabled();
    assert!(!state.is_available());
    assert!(state.is_disabled());
}

#[test]
fn test_cooldown_expiry() {
    let mut state = AuthState::cooldown(std::time::Duration::from_millis(10));
    assert!(!state.is_available());
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert!(state.maybe_recover());
    assert!(state.is_available());
}

#[test]
fn test_default_is_ready() {
    let state = AuthState::default();
    assert!(state.is_available());
}
