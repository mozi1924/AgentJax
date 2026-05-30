use crate::tools::ToolCatalogStateChange;
use serde_json::Value;
use std::time::Instant;

pub(in crate::runtime) struct ExecutedToolBatch {
    pub(in crate::runtime) tool_results_items: Vec<Value>,
    pub(in crate::runtime) executed_tool_call_items: Vec<Value>,
    pub(in crate::runtime) timeline_events: Vec<Value>,
    pub(in crate::runtime) state_changes: Vec<ToolCatalogStateChange>,
}

pub(super) struct ExecutedToolRecord {
    pub(super) index: usize,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) args: Value,
    pub(super) signature: String,
    pub(super) output_str: String,
    pub(super) is_success: bool,
    pub(super) started_at_unix_ms: i64,
    pub(super) completed_at_unix_ms: i64,
    pub(super) duration_ms: u64,
    pub(super) state_changes: Vec<ToolCatalogStateChange>,
}

pub(super) struct PreparedToolExecution {
    pub(super) index: usize,
    pub(super) call_id: String,
    pub(super) name: String,
    pub(super) args: Value,
    pub(super) signature: String,
    pub(super) repeated_fail_count: usize,
    pub(super) is_repeated_failure_guarded: bool,
}

pub(super) struct ActiveToolExecution {
    pub(super) name: String,
    pub(super) started_at: Instant,
}
