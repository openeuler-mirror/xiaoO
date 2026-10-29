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

#[tokio::test]
async fn cancelling_a_blocking_writer_waiter_prevents_its_operation() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let permit = Acquire(Some(global().request(Group::Spawn))).await;
    let executed = Arc::new(AtomicBool::new(false));
    {
        let mut future = Box::pin(write_blocking({
            let executed = Arc::clone(&executed);
            move || executed.store(true, Ordering::SeqCst)
        }));
        tokio::select! {
            result = &mut future => panic!("writer must wait: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(20)) => {},
        }
    }
    drop(permit);
    // A subsequent writer waits behind any still-unwinding cancelled request.
    assert_eq!(write_blocking(|| 42).await.unwrap(), Some(42));
    assert!(!executed.load(Ordering::SeqCst));
}

#[cfg(unix)]
#[tokio::test]
async fn expired_ready_admission_never_enters_the_child() {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    let (mut observation, signal) = UnixStream::pair().unwrap();
    observation.set_nonblocking(true).unwrap();
    let descriptor = signal.as_raw_fd();
    let mut command = tokio::process::Command::new("/bin/true");
    unsafe {
        command.pre_exec(move || {
            // write(2) is async-signal-safe and proves fork happened even if the
            // child is killed before exec or before creating a marker file.
            libc::write(descriptor, b"x".as_ptr().cast(), 1);
            Ok(())
        });
    }
    let writer = write_permit().await;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(20);
    let mut future = Box::pin(spawn_tokio_with_deadline(&mut command, Some(deadline)));
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    // The runtime is deliberately unable to poll its timer until both expiry
    // and group admission are ready. An outer timeout alone polls inner first.
    std::thread::sleep(Duration::from_millis(30));
    drop(writer);
    let error = future.await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    let mut byte = [0];
    assert_eq!(
        observation.read(&mut byte).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    drop(signal);
}

#[tokio::test]
async fn nested_deadline_scopes_preserve_the_earlier_deadline() {
    let earlier = tokio::time::Instant::now() + Duration::from_millis(10);
    with_deadline(earlier, async {
        with_deadline(earlier + Duration::from_secs(5), async {
            assert_eq!(current_deadline(), Some(earlier));
        })
        .await;
    })
    .await;
    assert!(current_deadline().is_none());
}
