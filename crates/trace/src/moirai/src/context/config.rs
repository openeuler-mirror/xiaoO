const DEFAULT_BUFFER_SIZE: usize = 100;

pub struct ContextConfig {
    pub buffer_size: usize,
    pub immediate_flush: bool,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            buffer_size: DEFAULT_BUFFER_SIZE,
            immediate_flush: false,
        }
    }
}
