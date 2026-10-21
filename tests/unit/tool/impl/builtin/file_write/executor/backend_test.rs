use agent_contracts::tool::source::DiscoveredTool;
use agent_contracts::ToolSource;
use agent_types::tool::{FinalToolCall, RawToolOutcome, ToolExecutorOutput};
use serde_json::json;

use crate::r#impl::builtin::test_support::RecordedMutation;
use crate::r#impl::builtin::test_support::{record_extra_forwarding, BackendTestRuntime};
use crate::r#impl::builtin::BuiltinToolSource;
use crate::r#impl::ToolRuntimeServices;

fn call_with_extra(input: serde_json::Value, extra: serde_json::Value) -> FinalToolCall {
    FinalToolCall {
        call_id: "file-write-call".to_string(),
        tool_name: "file_write".to_string(),
        input,
        extra: Some(extra),
        ..Default::default()
    }
}

fn extra_channel() -> serde_json::Value {
    json!([
        {
            "hooker_id": "plugin-x",
            "spawn_wrapper": {
                "prefix": ["/opt/dummy-wrapper", "--session=s1", "--"],
                "externalize_fs_writes": true
            }
        }
    ])
}

fn tools() -> Vec<DiscoveredTool> {
    BuiltinToolSource::new(ToolRuntimeServices::default()).discover()
}

/// file_write forwards `FinalToolCall.extra` to every mutation-class backend
/// call: the write_bytes request of an update, with no create_dir_all (the
/// file already exists).
#[tokio::test]
async fn file_write_update_forwards_extra_to_write_bytes() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("existing.txt"), "old\n").expect("seed file");
    let local = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
        .expect("local backend");
    let (backend, recorded) = record_extra_forwarding(local);
    let runtime = BackendTestRuntime::new(backend);

    let executor = tools()
        .into_iter()
        .find(|tool| tool.spec.name().0 == "file_write")
        .expect("file_write tool");

    let output = executor
        .executor
        .invoke(
            &call_with_extra(
                json!({ "file_path": "existing.txt", "content": "new\n" }),
                extra_channel(),
            ),
            &runtime,
        )
        .await
        .expect("file_write execution");
    assert!(matches!(
        output,
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { .. }
        }
    ));

    let recorded = recorded.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        1,
        "update writes once, no mkdir: {recorded:?}"
    );
    match &recorded[0] {
        RecordedMutation::WriteBytes { path, extra } => {
            assert!(path.ends_with("existing.txt"));
            assert_eq!(extra.as_ref(), Some(&extra_channel()));
        }
        other => panic!("expected a write_bytes mutation, got {other:?}"),
    }
}

/// Creating a file under a missing directory must forward `extra` to BOTH the
/// parent-directory creation and the write itself.
#[tokio::test]
async fn file_write_create_forwards_extra_to_mkdir_and_write() {
    let temp = tempfile::tempdir().expect("tempdir");
    let local = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
        .expect("local backend");
    let (backend, recorded) = record_extra_forwarding(local);
    let runtime = BackendTestRuntime::new(backend);

    let executor = tools()
        .into_iter()
        .find(|tool| tool.spec.name().0 == "file_write")
        .expect("file_write tool");

    let output = executor
        .executor
        .invoke(
            &call_with_extra(
                json!({ "file_path": "nested/dir/new.txt", "content": "hello\n" }),
                extra_channel(),
            ),
            &runtime,
        )
        .await
        .expect("file_write execution");
    assert!(matches!(
        output,
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { .. }
        }
    ));

    let recorded = recorded.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "mkdir + write: {recorded:?}");
    match &recorded[0] {
        RecordedMutation::CreateDirAll { path, extra } => {
            assert!(path.ends_with("nested/dir"), "unexpected mkdir path {path}");
            assert_eq!(extra.as_ref(), Some(&extra_channel()));
        }
        other => panic!("expected a create_dir_all mutation first, got {other:?}"),
    }
    match &recorded[1] {
        RecordedMutation::WriteBytes { path, extra } => {
            assert!(path.ends_with("nested/dir/new.txt"));
            assert_eq!(extra.as_ref(), Some(&extra_channel()));
        }
        other => panic!("expected a write_bytes mutation second, got {other:?}"),
    }
}

/// Without plugin contributions (`extra: None`) the mutation requests carry
/// no extra — the structural "not enabled, zero change" guarantee.
#[tokio::test]
async fn file_write_without_extra_yields_requests_without_extra() {
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::write(temp.path().join("plain.txt"), "x\n").expect("seed file");
    let local = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
        .expect("local backend");
    let (backend, recorded) = record_extra_forwarding(local);
    let runtime = BackendTestRuntime::new(backend);

    let executor = tools()
        .into_iter()
        .find(|tool| tool.spec.name().0 == "file_write")
        .expect("file_write tool");

    let plain_call = FinalToolCall {
        call_id: "plain".to_string(),
        tool_name: "file_write".to_string(),
        input: json!({ "file_path": "plain.txt", "content": "y\n" }),
        ..Default::default()
    };
    executor
        .executor
        .invoke(&plain_call, &runtime)
        .await
        .expect("file_write execution");

    let recorded = recorded.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1);
    match &recorded[0] {
        RecordedMutation::WriteBytes { extra, .. } => assert!(extra.is_none()),
        other => panic!("expected a write_bytes mutation, got {other:?}"),
    }
}
