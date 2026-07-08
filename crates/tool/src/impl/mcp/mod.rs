//! MCP-backed tool source: surfaces tools from connected MCP servers through
//! the standard `ToolSource` extension point.

mod executor;
mod spec;
mod tool_source;

pub use tool_source::McpToolSource;
