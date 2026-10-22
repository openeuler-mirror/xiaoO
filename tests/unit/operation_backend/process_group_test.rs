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
    if let Some(marker) = std::env::var_os(CHILD_ENV) {
        use std::io::Read;
        assert_eq!(
            std::env::args().skip(1).collect::<Vec<_>>(),
            [
                "--exact",
                "process_group::tests::kill_all_process_groups_clears_registry",
                "--nocapture"
            ],
            "cleanup body requires an isolated single-test invocation"
        );
        let marker = marker.to_str().expect("valid isolated cleanup marker");
        let (parent, nonce) = marker
            .split_once(':')
            .expect("invalid isolated cleanup marker");
        assert_eq!(parent.parse::<u32>().unwrap(), unsafe { libc::getppid() }
            as u32);
        let mut challenge = String::new();
        std::io::stdin().read_to_string(&mut challenge).unwrap();
        assert!(
            !nonce.is_empty() && challenge == nonce,
            "isolated cleanup challenge mismatch"
        );
    } else {
        run_isolated_cleanup("process_group::tests::kill_all_process_groups_clears_registry");
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

#[cfg(unix)]
fn run_isolated_cleanup(filter: &str) {
    use std::hash::{BuildHasher, Hasher};
    use std::io::Write;
    use std::process::Stdio;

    assert!(
        std::env::var_os("XIAOO_ISOLATED_PROCESS_GROUP_TEST").is_none(),
        "unexpected isolated cleanup marker"
    );
    let nonce = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
        .to_string();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", filter, "--nocapture"])
        .env(
            "XIAOO_ISOLATED_PROCESS_GROUP_TEST",
            format!("{}:{nonce}", std::process::id()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run cleanup test in a separate process");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(nonce.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "isolated cleanup test failed: {stdout}\n{stderr}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line == format!("test {filter} ... ok")),
        "target test did not pass: {stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")),
        "expected exactly one executed test: {stdout}"
    );
}

#[test]
#[cfg(unix)]
fn isolated_cleanup_rejects_a_mismatched_filter() {
    assert!(std::panic::catch_unwind(|| run_isolated_cleanup(
        "process_group::tests::missing_cleanup_test"
    ))
    .is_err());
}

#[test]
#[cfg(unix)]
fn isolated_cleanup_rejects_an_external_marker() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "process_group::tests::kill_all_process_groups_clears_registry",
            "--nocapture",
        ])
        .env("XIAOO_ISOLATED_PROCESS_GROUP_TEST", "preexisting")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid isolated cleanup marker"));
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
