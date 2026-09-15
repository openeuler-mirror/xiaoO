use super::*;

impl PluginHookerCore {
    /// Replace the hook point. Used by adaptor unit tests to point a shared
    /// `adaptor_for` helper at the specific hook point under test.
    pub(crate) fn set_hook_point(&mut self, hook_point: HookPointId) {
        self.hook_point = hook_point;
    }
}
