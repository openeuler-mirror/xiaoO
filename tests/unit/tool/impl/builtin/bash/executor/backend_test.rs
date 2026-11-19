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

/// Runs `enforce_session_budget` over spills created with the given
/// `(age_secs, size_bytes)` pairs (index 0 = oldest) and returns the
/// indexes that survived eviction. `keep` is the spill index that must
/// never be evicted.
fn survivors_after_budget(spills: &[(u64, u64)], keep: usize, budget: u64) -> Vec<usize> {
    let dir = tempfile::tempdir().expect("tempdir");
    let now = SystemTime::now();
    for (index, (age, size)) in spills.iter().enumerate() {
        make_spill(
            dir.path(),
            &format!(".xiaoo-bash-output-s-{}.txt", index + 1),
            *size,
            now - Duration::from_secs(*age),
        );
    }
    enforce_session_budget(
        dir.path(),
        ".xiaoo-bash-output-s-",
        std::ffi::OsStr::new(&format!(".xiaoo-bash-output-s-{}.txt", keep + 1)),
        budget,
    );
    (0..spills.len())
        .filter(|index| {
            dir.path()
                .join(format!(".xiaoo-bash-output-s-{}.txt", index + 1))
                .exists()
        })
        .collect()
}

/// The budget evicts the oldest spills first, never the kept file (even
/// when it is the oldest), and is a no-op under the limit.
#[test]
fn budget_evicts_oldest_spills_but_never_the_kept_one() {
    // Three 60-byte spills, budget 100 fits one: the two oldest are evicted,
    // the freshest (also the kept one) survives.
    assert_eq!(
        survivors_after_budget(&[(30, 60), (20, 60), (10, 60)], 2, 100),
        vec![2],
        "oldest spills should be evicted, freshest kept"
    );
    // The kept file is the oldest: it must survive anyway, and the younger
    // non-kept spills are evicted to honour the budget.
    assert_eq!(
        survivors_after_budget(&[(30, 60), (20, 60), (10, 60)], 0, 100),
        vec![0],
        "kept file must never be evicted"
    );
    // Under the budget nothing is evicted.
    assert_eq!(
        survivors_after_budget(&[(20, 30)], 0, 100),
        vec![0],
        "no files should be evicted under budget"
    );
}

#[test]
fn reclaim_skips_non_host_paths() {
    // A sandbox-style path (e.g. E2B) does not exist on the host, so the
    // reclaimer must be a no-op and never inspect the filesystem.
    reclaim_host_spills("/tmp/.xiaoo-bash-output-e2b-1.txt", "e2b");
}

#[tokio::test]
async fn client_stdout_stays_structured_after_escape_expansion() {
    use agent_contracts::backend::capability::exec::ExecResult;

    let temp = tempfile::tempdir().unwrap();
    let backend =
        operation_backend::local_backend(temp.path().to_path_buf(), None, None, None).unwrap();
    let output = BashExecutor::format_output(
        &*backend,
        &ExecResult {
            stdout: vec![0x1b; 16 * 1024],
            exit_code: Some(7),
            output: ExecOutput::Combined { truncated: false },
            ..Default::default()
        },
        "test-session",
    )
    .await;
    let json = serde_json::to_string(&output).unwrap();
    assert!(json.len() < crate::r#impl::builtin::CLIENT_RESULT_LIMIT);
    let value: BashOutput = serde_json::from_str(&json).unwrap();
    assert!(value.stdout_truncated && !value.stdout.is_empty());
    assert_eq!(value.exit_code, Some(7));
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}
