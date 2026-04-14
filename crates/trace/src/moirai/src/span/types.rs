//! Span type definition.
//!
//! SpanType is a string to allow external crates to define custom span types.

/// Span type as string, allows external crates to define custom types.
pub type SpanType = String;

/// Built-in span type constants for core operations.
pub mod span_types {
    /// Root span for a user-initiated agent
    pub const USER: &str = "USER";
    /// Root span for a quest (long-running task)
    pub const QUEST: &str = "QUEST";
    /// Root span for a spawned child agent
    pub const SPAWNED: &str = "SPAWNED";
    /// Span recording a spawn operation
    pub const SPAWN: &str = "SPAWN";
    /// Span recording a merge operation
    pub const MERGE: &str = "MERGE";
    /// Span recording the end of a trace
    pub const END: &str = "END";
}
