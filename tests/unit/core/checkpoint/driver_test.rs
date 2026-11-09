use super::CheckpointDriver;

impl CheckpointDriver {
    pub(in crate::checkpoint) async fn is_hot(&self) -> bool {
        self.inner
            .lock()
            .await
            .as_ref()
            .is_some_and(|state| state.hot)
    }
}
