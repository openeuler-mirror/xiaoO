//! Regression for the daemon-form `create_dir_all` externalization gap
//! (phase-6 XR15): the trait's default method silently drops the plugin
//! `extra` channel, keeping new-file parent mkdirs inside the host process
//! with stale session attribution. Both gateway wrappers are pinned with a
//! recording inner backend that captures the extra it received on
//! `create_dir_all_with_extra` (the recording pattern of the tool executor
//! suite); the subprocess routing the forwarded extra drives is already
//! covered by the externalization suite in operation_backend.

use super::*;
use crate::gateway::permission_backend::PermissionAwareOperationBackend;
use agent_contracts::backend::capability::export::OperationExport;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;

/// The plugin extra channel the tests forward: a spawn_wrapper whose
/// `externalize_fs_writes` is off, so the recording layer's in-process
/// delegation cannot trip the externalized path — the wrapper layers under
/// test are pure pass-throughs and never inspect the value.
fn wrapper_extra() -> serde_json::Value {
    serde_json::json!([
        {
            "hooker_id": "plugin-x",
            "spawn_wrapper": {
                "prefix": ["/opt/dummy-wrapper", "--session=s1", "--"],
                "externalize_fs_writes": false
            }
        }
    ])
}

/// The `create_dir_all_with_extra` calls that reached the recording layer.
#[derive(Default)]
struct ForwardedMkDirs {
    calls: Mutex<Vec<(String, Option<serde_json::Value>)>>,
}

impl ForwardedMkDirs {
    /// The one recorded call; panics unless exactly one arrived.
    fn single_call(&self) -> (String, Option<serde_json::Value>) {
        let calls = self.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "exactly one mkdir must be forwarded: {calls:?}"
        );
        calls[0].clone()
    }
}

/// Wraps a real backend, recording every `create_dir_all_with_extra` the
/// outer gateway layers forward; everything else delegates unchanged.
struct ExtraRecordingBackend {
    inner: Arc<dyn OperationBackend>,
    files: ExtraRecordingFileSystem,
}

impl ExtraRecordingBackend {
    fn wrap(
        inner: Arc<dyn OperationBackend>,
    ) -> (Arc<dyn OperationBackend>, Arc<ForwardedMkDirs>) {
        let forwarded = Arc::new(ForwardedMkDirs::default());
        (
            Arc::new(Self {
                files: ExtraRecordingFileSystem {
                    inner: Arc::clone(&inner),
                    forwarded: Arc::clone(&forwarded),
                },
                inner,
            }),
            forwarded,
        )
    }
}

struct ExtraRecordingFileSystem {
    inner: Arc<dyn OperationBackend>,
    forwarded: Arc<ForwardedMkDirs>,
}

#[async_trait]
impl OperationFileSystem for ExtraRecordingFileSystem {
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        self.inner.files().stat(path).await
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        self.inner.files().read_bytes(request).await
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        self.inner.files().write_bytes(request).await
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        self.inner.files().create_dir_all(path).await
    }

    async fn create_dir_all_with_extra(
        &self,
        path: &BackendPath,
        extra: Option<&serde_json::Value>,
    ) -> Result<(), OperationError> {
        self.forwarded
            .calls
            .lock()
            .unwrap()
            .push((path.native().to_string(), extra.cloned()));
        self.inner
            .files()
            .create_dir_all_with_extra(path, extra)
            .await
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        self.inner.files().temp_path(request).await
    }
}

#[async_trait]
impl OperationBackend for ExtraRecordingBackend {
    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        self.inner.capabilities()
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        self.inner.paths()
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files
    }

    fn search(&self) -> &dyn OperationSearch {
        self.inner.search()
    }

    fn exec(&self) -> &dyn OperationExec {
        self.inner.exec()
    }

    fn export(&self) -> &dyn OperationExport {
        self.inner.export()
    }

    fn attach_interaction(&self, interaction: Arc<dyn InteractionHandle>) {
        self.inner.attach_interaction(interaction);
    }

    fn permission_control(&self) -> Option<&dyn OperationPermissionControl> {
        self.inner.permission_control()
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        self.inner.shutdown().await
    }
}

struct UnusedInteraction;

#[async_trait]
impl InteractionHandle for UnusedInteraction {
    async fn ask(&self, _request: &InteractionRequest) -> InteractionResponse {
        panic!("interaction is not used for create_dir_all")
    }
}

/// Drive one wrapped `create_dir_all_with_extra` call.
fn create_dir_all_through(
    wrapped: &dyn OperationBackend,
    target: &std::path::Path,
) -> Result<(), OperationError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(wrapped.files().create_dir_all_with_extra(
            &BackendPath::from_raw(target.display().to_string()),
            Some(&wrapper_extra()),
        ))
}

fn local_backend_in(workspace: &std::path::Path) -> Arc<dyn OperationBackend> {
    operation_backend::local_backend(workspace.to_path_buf(), None, None, None)
        .expect("local backend")
}

#[test]
fn dirty_tracked_backend_forwards_create_dir_all_extra() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (recording, forwarded) = ExtraRecordingBackend::wrap(local_backend_in(&workspace));
    let tracker = Arc::new(BackendDirtyTracker::default());
    let wrapped = DirtyTrackedOperationBackend::wrap(recording, Arc::clone(&tracker));

    let target = workspace.join("nested/dir");
    create_dir_all_through(wrapped.as_ref(), &target).expect("wrapped mkdir succeeds");

    assert_eq!(
        forwarded.single_call(),
        (target.display().to_string(), Some(wrapper_extra())),
        "the dirty layer must forward the plugin extra channel untouched"
    );
    assert!(target.is_dir(), "the recording layer delegated the mkdir");
    assert!(
        tracker.is_dirty(),
        "the forwarded mkdir still marks the backend dirty"
    );
}

#[test]
fn permission_aware_backend_forwards_create_dir_all_extra() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let (recording, forwarded) = ExtraRecordingBackend::wrap(local_backend_in(&workspace));
    let wrapped =
        PermissionAwareOperationBackend::new(recording, Arc::new(UnusedInteraction), None);

    let target = workspace.join("other/dir");
    create_dir_all_through(&wrapped, &target).expect("wrapped mkdir succeeds");

    assert_eq!(
        forwarded.single_call(),
        (target.display().to_string(), Some(wrapper_extra())),
        "the permission layer must forward the plugin extra channel untouched"
    );
    assert!(target.is_dir(), "the recording layer delegated the mkdir");
}
