use agent_contracts::trace::TraceSpanHandle;
use agent_types::tool::{
    ErrorHookResult, FinalToolCall, PostHookResult, PreHookResult, RawToolOutcome,
    ToolExecutionError, ToolLifecycleRecord,
};

pub(super) struct ToolExecutionState {
    pub(super) final_call: FinalToolCall,
    pub(super) trace_span: Option<TraceSpanHandle>,
    pub(super) lifecycle_record: Option<ToolLifecycleRecord>,
    pub(super) pre_hook_results: Vec<PreHookResult>,
    pub(super) post_hook_results: Vec<PostHookResult>,
    pub(super) error_hook_results: Vec<ErrorHookResult>,
    pub(super) raw_outcome: Option<RawToolOutcome>,
    pub(super) execution_error: Option<ToolExecutionError>,
}

pub(super) enum ExecutorPhaseResult {
    Completed(RawToolOutcome),
    Suspended(String),
    Failed(ToolExecutionError),
}
