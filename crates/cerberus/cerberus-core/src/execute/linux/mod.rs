//! Linux-specific execution implementation.

mod error;
mod namespace_sync;
mod pipe;
mod sandbox_exec;
mod wait;

pub(in crate::execute) use sandbox_exec::execute_process_linux;
