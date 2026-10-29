use super::*;

impl PluginHookerCore {
    /// Replace the hook point. Used by adaptor unit tests to point a shared
    /// `adaptor_for` helper at the specific hook point under test.
    pub(crate) fn set_hook_point(&mut self, hook_point: HookPointId) {
        self.hook_point = hook_point;
    }
}

#[cfg(unix)]
#[tokio::test]
async fn expired_ready_admission_does_not_complete_plugin_hook() {
    use std::future::Future;
    let writer = process_coordination::write_permit().await;
    let id = HookerId("deadline-hook".into());
    let payload = serde_json::json!({});
    let make_error = |message: String| format!("execution: {message}");
    let make_timeout = |message: String, _| format!("timeout: {message}");
    let mut call = Box::pin(run_plugin_subprocess(
        &id,
        "printf '{}'",
        &payload,
        &make_error,
        &make_timeout,
        Some(20),
    ));
    std::future::poll_fn(|cx| {
        assert!(call.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    std::thread::sleep(std::time::Duration::from_millis(30));
    drop(writer);
    assert!(call.await.unwrap_err().starts_with("timeout:"));
}
