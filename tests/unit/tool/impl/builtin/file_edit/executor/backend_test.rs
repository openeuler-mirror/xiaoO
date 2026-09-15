use std::time::Duration;

use agent_contracts::ToolSource;
use agent_types::tool::{FinalToolCall, RawToolOutcome, ToolExecutorOutput};
use serde_json::json;

use crate::r#impl::builtin::test_support::{override_atomic_write_capability, BackendTestRuntime};
use crate::r#impl::builtin::BuiltinToolSource;
use crate::r#impl::ToolRuntimeServices;

fn call(tool_name: &str, input: serde_json::Value) -> FinalToolCall {
    FinalToolCall {
        call_id: format!("{tool_name}-call"),
        tool_name: tool_name.to_string(),
        input,
        ..Default::default()
    }
}

#[tokio::test]
async fn rejects_edit_when_file_changed_after_read() {
    const ORIGINAL: &str = "fn main() {\n    let timeout = 30;\n}\n";
    const USER_EDIT: &str = "fn main() {\n    let timeout = 30;\n    enable_tls();\n}\n";

    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("sample.rs");
    std::fs::write(&file_path, ORIGINAL).expect("write initial file");
    let read_mtime = std::fs::metadata(&file_path)
        .and_then(|metadata| metadata.modified())
        .expect("initial mtime");

    let backend = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
        .expect("local backend");
    let runtime = BackendTestRuntime::new(backend);

    // Use the production source so both executors receive its private state.
    let tools = BuiltinToolSource::new(ToolRuntimeServices::default()).discover();
    let read_executor = tools
        .iter()
        .find(|tool| tool.spec.name().0 == "file_read")
        .expect("file_read tool");
    let edit_executor = tools
        .iter()
        .find(|tool| tool.spec.name().0 == "file_edit")
        .expect("file_edit tool");

    let read_output = read_executor
        .executor
        .invoke(
            &call("file_read", json!({ "file_path": "sample.rs" })),
            &runtime,
        )
        .await
        .expect("file_read execution");
    assert!(matches!(
        read_output,
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { .. }
        }
    ));

    // Simulate a user edit while keeping old_string present. This isolates
    // the stale-read guard from the ordinary "old_string not found" check.
    std::fs::write(&file_path, USER_EDIT).expect("modify file after read");

    // Advance mtime explicitly instead of sleeping for the filesystem clock.
    std::fs::File::options()
        .write(true)
        .open(&file_path)
        .and_then(|file| file.set_modified(read_mtime + Duration::from_secs(1)))
        .expect("advance modified time");

    let edit_output = edit_executor
        .executor
        .invoke(
            &call(
                "file_edit",
                json!({
                    "file_path": "sample.rs",
                    "old_string": "let timeout = 30;",
                    "new_string": "let timeout = 60;"
                }),
            ),
            &runtime,
        )
        .await
        .expect("file_edit execution");

    match edit_output {
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Error { message },
        } => assert!(
            message.starts_with("[error_code=7]"),
            "expected FILE_MODIFIED, got: {message}"
        ),
        other => panic!("expected file_edit to reject stale edit, got: {other:?}"),
    }

    // A rejected edit must leave the user's version untouched.
    assert_eq!(
        std::fs::read_to_string(file_path).expect("read final file"),
        USER_EDIT
    );
}

#[tokio::test]
async fn creates_and_edits_files_without_atomic_write_support() {
    let temp = tempfile::tempdir().expect("tempdir");
    let backend = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
        .expect("local backend");
    let backend = override_atomic_write_capability(backend, false);
    let runtime = BackendTestRuntime::new(backend);

    let tools = BuiltinToolSource::new(ToolRuntimeServices::default()).discover();
    let edit_executor = tools
        .iter()
        .find(|tool| tool.spec.name().0 == "file_edit")
        .expect("file_edit tool");

    let create_output = edit_executor
        .executor
        .invoke(
            &call(
                "file_edit",
                json!({
                    "file_path": "sample.txt",
                    "old_string": "",
                    "new_string": "hello\n"
                }),
            ),
            &runtime,
        )
        .await
        .expect("file_edit create execution");
    assert!(matches!(
        create_output,
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { .. }
        }
    ));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("sample.txt")).expect("read created file"),
        "hello\n"
    );

    let edit_output = edit_executor
        .executor
        .invoke(
            &call(
                "file_edit",
                json!({
                    "file_path": "sample.txt",
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
            ),
            &runtime,
        )
        .await
        .expect("file_edit update execution");
    assert!(matches!(
        edit_output,
        ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { .. }
        }
    ));
    assert_eq!(
        std::fs::read_to_string(temp.path().join("sample.txt")).expect("read edited file"),
        "goodbye\n"
    );
}
