use super::*;
use crate::test_support::process_group_test_lock;

#[test]
#[cfg(unix)]
fn test_register_unregister_pgid() {
    let _guard = process_group_test_lock().lock().unwrap();
    register_pgid(12345);
    {
        if let Ok(set) = ACTIVE_PGIDS.lock() {
            assert!(set.contains(&12345));
        }
    }
    unregister_pgid(12345);
    {
        if let Ok(set) = ACTIVE_PGIDS.lock() {
            assert!(!set.contains(&12345));
        }
    }
}

#[test]
#[cfg(unix)]
fn test_kill_all_process_groups() {
    let _guard = process_group_test_lock().lock().unwrap();
    register_pgid(11111);
    register_pgid(22222);
    {
        if let Ok(set) = ACTIVE_PGIDS.lock() {
            assert!(set.contains(&11111));
            assert!(set.contains(&22222));
        }
    }
    kill_all_process_groups();
    {
        if let Ok(set) = ACTIVE_PGIDS.lock() {
            assert!(set.is_empty());
        }
    }
}

#[test]
fn test_terminate_invalid_pgid() {
    let result = terminate_process_group(-1);
    #[cfg(unix)]
    assert!(result.is_err());
}
