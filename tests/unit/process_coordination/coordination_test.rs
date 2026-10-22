use super::*;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn admitted(c: &Coordinator, request: &Request<'_>) -> bool {
    c.state.lock().unwrap().entries[&request.id].admitted
}

#[test]
fn opposite_waiter_seals_and_switches_finite_concurrent_batches() {
    let c = Coordinator::default();
    let w1 = c.request(Group::Write);
    let w2 = c.request(Group::Write);
    assert!(admitted(&c, &w1) && admitted(&c, &w2));
    let s1 = c.request(Group::Spawn);
    let s2 = c.request(Group::Spawn);
    let w3 = c.request(Group::Write);
    assert!(!admitted(&c, &s1) && !admitted(&c, &w3));
    drop(w1);
    assert!(!admitted(&c, &s1));
    drop(w2);
    assert!(admitted(&c, &s1) && admitted(&c, &s2));
    let s3 = c.request(Group::Spawn);
    assert!(!admitted(&c, &s3));
    drop(s1);
    drop(s2);
    assert!(admitted(&c, &w3) && !admitted(&c, &s3));
    let w4 = c.request(Group::Write);
    drop(w3);
    assert!(admitted(&c, &s3) && !admitted(&c, &w4));
    drop(s3);
    assert!(admitted(&c, &w4));
    drop(w4);
    assert!(c.state.lock().unwrap().entries.is_empty());
}

#[tokio::test]
async fn cancelled_waiter_and_unpolled_admission_release_their_slots() {
    let c = Coordinator::default();
    let write = c.request(Group::Write);
    let pending = Acquire(Some(c.request(Group::Spawn)));
    assert!(tokio::time::timeout(Duration::from_millis(10), pending)
        .await
        .is_err());
    let admitted_later = c.request(Group::Spawn);
    let writer_later = c.request(Group::Write);
    drop(write);
    assert!(admitted(&c, &admitted_later));
    // Cancellation after batch admission, before the future is polled again.
    drop(admitted_later);
    assert!(admitted(&c, &writer_later));
    drop(writer_later);
    assert!(c.state.lock().unwrap().entries.is_empty());
}

#[test]
fn unwinding_and_error_paths_release_permits() {
    let c = Coordinator::default();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _permit = Permit {
            _request: c.request(Group::Write),
        };
        panic!("writer failed");
    }));
    assert!(panic.is_err());
    let result = (|| -> std::io::Result<()> {
        let _permit = Permit {
            _request: c.request(Group::Spawn),
        };
        Err(std::io::Error::other("spawn failed"))
    })();
    assert!(result.is_err());
    let write = c.request(Group::Write);
    assert!(admitted(&c, &write));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuous_load_makes_progress_in_both_groups() {
    let c = Arc::new(Coordinator::default());
    let mut tasks = Vec::new();
    for group in [Group::Write, Group::Spawn, Group::Write, Group::Spawn] {
        let c = Arc::clone(&c);
        tasks.push(tokio::spawn(async move {
            let mut longest = Duration::ZERO;
            for _ in 0..200 {
                let start = Instant::now();
                let permit = Acquire(Some(c.request(group))).await;
                longest = longest.max(start.elapsed());
                {
                    let state = c.state.lock().unwrap();
                    assert!(state
                        .entries
                        .values()
                        .filter(|entry| entry.admitted)
                        .all(|entry| entry.group == group));
                }
                tokio::task::yield_now().await;
                drop(permit);
            }
            longest
        }));
    }
    for task in tasks {
        let longest = tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        println!("observed maximum permit wait: {longest:?}");
    }
    assert!(c.state.lock().unwrap().entries.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn spawned_child_runtime_does_not_hold_creation_permit() {
    let mut child = spawn_tokio(
        tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .kill_on_drop(true),
    )
    .await
    .unwrap();
    let permit = tokio::time::timeout(Duration::from_secs(1), write_permit())
        .await
        .unwrap();
    assert!(child.try_wait().unwrap().is_none());
    drop(permit);
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn spawn_error_releases_creation_permit() {
    assert!(
        spawn_tokio(&mut tokio::process::Command::new("/nonexistent/command"))
            .await
            .is_err()
    );
    drop(
        tokio::time::timeout(Duration::from_secs(1), write_permit())
            .await
            .unwrap(),
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn synchronous_admission_yields_its_runtime_worker() {
    let (started, ready) = tokio::sync::oneshot::channel();
    let writer = tokio::spawn(async move {
        let permit = write_permit().await;
        started.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(permit);
    });
    ready.await.unwrap();
    assert!(
        spawn_std_blocking(&mut std::process::Command::new("/bin/true"))
            .unwrap()
            .wait()
            .unwrap()
            .success()
    );
    writer.await.unwrap();
}
