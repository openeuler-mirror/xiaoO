use super::*;
use agent_contracts::backend::capability::filesystem::ReadBytesRequest;
use agent_contracts::backend::{OperationError, PathNamespace};

#[tokio::test]
async fn overflow_spills_via_backend_and_stays_readable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let backend = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
        .expect("local backend");

    // Exceed the per-stream window so the full output must be spilled.
    let bytes = vec![b'x'; MAX_OUTPUT_BYTES_PER_STREAM + 10_000];
    let (text, truncated) = window_stream(&*backend, "test-session", &bytes).await;
    assert!(truncated);

    // The model is told where the full output lives, so it can read it back.
    let marker = "the FULL output is saved at ";
    let start = text
        .find(marker)
        .unwrap_or_else(|| panic!("expected spill marker in: {text}"))
        + marker.len();
    let spill = text[start..]
        .split('—')
        .next()
        .expect("spill path in message")
        .trim();
    assert!(!spill.is_empty());

    let full = backend
        .files()
        .read_bytes(ReadBytesRequest {
            path: BackendPath::from_raw(spill.to_string()),
        })
        .await
        .expect("spill file readable through the backend");
    assert_eq!(
        full, bytes,
        "full output must round-trip through the backend"
    );

    // A path claiming a different backend must be rejected so cross-backend
    // paths can never be interpreted inside this one.
    let foreign = BackendPath::new(
        "some-other-backend",
        PathNamespace::Temp,
        spill.to_string(),
        "spill.txt",
    );
    let denied = backend
        .files()
        .read_bytes(ReadBytesRequest { path: foreign })
        .await;
    assert!(
        matches!(denied, Err(OperationError::PermissionDenied { .. })),
        "expected PermissionDenied, got {denied:?}"
    );
}

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn make_spill(dir: &Path, name: &str, size: u64, mtime: SystemTime) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create spill");
    file.set_len(size).expect("set_len");
    file.set_modified(mtime).expect("set_modified");
    path
}

#[test]
fn budget_evicts_oldest_first_keeping_freshest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let now = SystemTime::now();
    make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-1.txt",
        60,
        now - Duration::from_secs(30),
    );
    make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-2.txt",
        60,
        now - Duration::from_secs(20),
    );
    let fresh = make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-3.txt",
        60,
        now - Duration::from_secs(10),
    );

    enforce_session_budget(
        dir.path(),
        ".xiaoo-bash-output-s-",
        std::ffi::OsStr::new(".xiaoo-bash-output-s-3.txt"),
        100,
    );

    assert!(
        !dir.path().join(".xiaoo-bash-output-s-1.txt").exists(),
        "oldest spill should be evicted"
    );
    assert!(
        !dir.path().join(".xiaoo-bash-output-s-2.txt").exists(),
        "next-oldest spill should be evicted"
    );
    assert!(fresh.exists(), "freshest spill must survive");
}

#[test]
fn budget_never_evicts_kept_file_even_when_oldest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let now = SystemTime::now();
    let keep = make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-1.txt",
        60,
        now - Duration::from_secs(30),
    );
    make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-2.txt",
        60,
        now - Duration::from_secs(20),
    );
    make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-3.txt",
        60,
        now - Duration::from_secs(10),
    );

    enforce_session_budget(
        dir.path(),
        ".xiaoo-bash-output-s-",
        std::ffi::OsStr::new(".xiaoo-bash-output-s-1.txt"),
        100,
    );

    assert!(keep.exists(), "kept file must never be evicted");
    assert!(
        !dir.path().join(".xiaoo-bash-output-s-2.txt").exists(),
        "non-kept spill should be evicted to honour budget"
    );
    assert!(
        !dir.path().join(".xiaoo-bash-output-s-3.txt").exists(),
        "non-kept spill should be evicted to honour budget"
    );
}

#[test]
fn budget_under_limit_is_noop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let now = SystemTime::now();
    let spill = make_spill(
        dir.path(),
        ".xiaoo-bash-output-s-1.txt",
        30,
        now - Duration::from_secs(20),
    );

    enforce_session_budget(
        dir.path(),
        ".xiaoo-bash-output-s-",
        std::ffi::OsStr::new(".xiaoo-bash-output-s-1.txt"),
        100,
    );

    assert!(spill.exists(), "no files should be evicted under budget");
}

#[test]
fn reclaim_skips_non_host_paths() {
    // A sandbox-style path (e.g. E2B) does not exist on the host, so the
    // reclaimer must be a no-op and never inspect the filesystem.
    reclaim_host_spills("/tmp/.xiaoo-bash-output-e2b-1.txt", "e2b");
}
