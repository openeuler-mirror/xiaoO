//! Moirai - Core tracing infrastructure for agent execution.
//!
//! This crate provides span storage and topology management:
//! - `Span` - Core data structure for tracing records
//! - `AgentContext` - Context for managing trace lifecycle
//! - `SpanStorage` - Trait for span persistence
//! - `SqliteStorage` - SQLite-based storage implementation
//!
//! ## Span Types
//!
//! SpanType is a string type, allowing external crates to define custom span types.
//! Built-in types are provided in the `span_types` module.
//!
//! ## Example
//!
//! ```rust,ignore
//! use moirai::{AgentContext, SqliteStorage, span_types};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let storage = Arc::new(SqliteStorage::new(":memory:").unwrap());
//!     let ctx = AgentContext::new_user("my-agent", storage).await.unwrap();
//!
//!     // Record a custom span
//!     let span_id = ctx.record_span("CUSTOM", serde_json::json!({"key": "value"})).await.unwrap();
//!
//!     // End the trace
//!     ctx.end(true, Some("done")).await.unwrap();
//! }
//! ```

pub mod context;
pub mod error;
pub mod span;
pub mod storage;

pub use context::{AgentContext, ContextConfig, SpawnResult};
pub use error::MoiraiError;
pub use span::{span_types, Span, SpanHandle, SpanType};
pub use storage::{SpanStorage, SqliteStorage, TraceSummary};

pub type Result<T> = std::result::Result<T, MoiraiError>;
