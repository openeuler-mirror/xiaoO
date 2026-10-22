use super::*;
use crate::test_support::process_group_test_lock;

#[test]
#[cfg(unix)]
fn registered_pgid_is_tracked_until_unregistered() {
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
fn kill_all_process_groups_clears_registry() {
    use std::os::unix::process::{CommandExt, ExitStatusExt};

    const CHILD_ENV: &str = "XIAOO_ISOLATED_PROCESS_GROUP_TEST";
    if std::env::var_os(CHILD_ENV).is_none() {
        // Global shutdown cleanup must run with a private registry, away from
        // commands owned by the other tests in this process.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process_group::tests::kill_all_process_groups_clears_registry",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .expect("run cleanup test in a separate process");
        assert!(
            output.status.success(),
            "isolated cleanup test failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let _guard = process_group_test_lock().lock().unwrap();
    let mut children: Vec<_> = (0..2)
        .map(|_| {
            std::process::Command::new("sleep")
                .arg("30")
                .process_group(0)
                .spawn()
                .expect("spawn owned process group")
        })
        .collect();
    for child in &children {
        register_pgid(child.id() as i32);
    }
    {
        if let Ok(set) = ACTIVE_PGIDS.lock() {
            assert_eq!(set.len(), children.len());
            for child in &children {
                assert!(set.contains(&(child.id() as i32)));
            }
        }
    }
    kill_all_process_groups();
    {
        if let Ok(set) = ACTIVE_PGIDS.lock() {
            assert!(set.is_empty());
        }
    }
    for child in &mut children {
        let status = child.wait().expect("reap owned process group");
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}

#[test]
#[cfg(unix)]
fn global_cleanup_test_preserves_concurrent_commands() {
    use std::os::unix::process::CommandExt;

    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .expect("spawn concurrent command");
    let pgid = child.id() as i32;
    register_pgid(pgid);

    let cleanup = std::panic::catch_unwind(kill_all_process_groups_clears_registry);
    let status = child.try_wait().expect("query concurrent command");
    let _ = child.kill();
    child.wait().expect("reap concurrent command");
    unregister_pgid(pgid);

    cleanup.expect("global cleanup test succeeds");
    assert!(
        status.is_none(),
        "the global cleanup test terminated another test's command: {status:?}"
    );
}

#[test]
fn terminating_invalid_pgid_returns_error() {
    let result = terminate_process_group(-1);
    #[cfg(unix)]
    assert!(result.is_err());
}
