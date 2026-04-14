pub mod noop;
#[cfg(feature = "sqlite")]
pub mod openai;
pub mod provider;

pub use noop::NoopEmbedding;
#[cfg(feature = "sqlite")]
pub use openai::OpenAiEmbedding;
pub use provider::EmbeddingProvider;
