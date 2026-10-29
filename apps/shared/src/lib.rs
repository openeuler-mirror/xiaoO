pub mod backend;
pub mod builtin_agent_roles;
pub mod channels;
pub mod config_identity;
pub mod cron;
pub mod custom_tool_support;
pub mod daemon_protocol;
pub mod gateway;
pub mod hook_support;
pub mod host_identity;
pub mod llm_secrets;
pub mod lsp_support;
pub mod mcp_support;
pub mod plan;
pub mod runtime_checkpoint;
pub mod session_diff;
pub mod skills_support;
pub mod text_bounds;

pub mod trace_support {
    pub use trace::{
        default_trace_db_path, inspect_recent_hook_executions, inspect_trace_database,
        HookExecutionDiagnosticSummary, TraceDiagnosticSummary,
    };
}

/// 共享 error.log 路径约定的门面转发:权威实现在 agent-types,endside
/// 受门面约束不能直接依赖底层 crate,必须经此转发获取。
pub use agent_types::common::error_log_path;

#[cfg(feature = "test-support")]
#[path = "../../../tests/unit/shared/testing.rs"]
pub mod testing;

pub use runtime_checkpoint::{
    RuntimeCheckoutRequest, RuntimeCheckoutResult, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, RuntimeCheckpointSnapshotDeleteRequest,
    RuntimeCheckpointSnapshotDeleteResult, RuntimeCheckpointSummary, RuntimeExecRequest,
    RuntimeExecResult, RuntimePauseRequest, RuntimePauseResult, RuntimeReadFileRequest,
    RuntimeReadFileResult, RuntimeRecord, RuntimeResumeRequest, RuntimeResumeResult,
    RuntimeWriteFileRequest, RuntimeWriteFileResult,
};
