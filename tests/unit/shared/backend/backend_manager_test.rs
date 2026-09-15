use super::*;

impl BackendManager {
    /// Introspection handle for the sandbox pool counter, used by the limit /
    /// reconciliation tests to seed and assert pool state directly (real
    /// e2b sandboxes need provider credentials unavailable in CI).
    pub(crate) fn sandbox_counter(&self) -> &SandboxCounter {
        &self.sandbox_counter
    }
}
