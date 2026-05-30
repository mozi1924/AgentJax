use crate::providers::types::{ProviderUsage, ProviderUsageRecord, ResponseStreamResult};
use serde_json::Value;

/// Collects output and usage across all provider hops in a single agent turn.
pub(super) struct TurnAccumulator {
    pub(super) last_response_id: String,
    pub(super) output_items: Vec<Value>,
    pub(super) timeline_events: Vec<Value>,
    pub(super) usage: Option<ProviderUsage>,
    pub(super) usage_hops: Vec<ProviderUsageRecord>,
}

impl TurnAccumulator {
    pub(super) fn new() -> Self {
        Self {
            last_response_id: String::new(),
            output_items: Vec::new(),
            timeline_events: Vec::new(),
            usage: None,
            usage_hops: Vec::new(),
        }
    }

    pub(super) fn record_hop(&mut self, response: &ResponseStreamResult) {
        if !response.response_id.is_empty() {
            self.last_response_id = response.response_id.clone();
        }
        self.output_items.extend(response.output_items.clone());
        if let Some(response_usage) = &response.usage {
            if let Some(total_usage) = &mut self.usage {
                total_usage.saturating_add(response_usage);
            } else {
                self.usage = Some(response_usage.clone());
            }
            self.usage_hops.push(ProviderUsageRecord {
                response_id: response.response_id.clone(),
                usage: response_usage.clone(),
            });
        }
    }

    /// Append all continuation items so the final result contains the complete
    /// tool-call timeline, not just assistant text.
    pub(super) fn absorb_continuation_batch(&mut self, items: &[Value]) {
        self.output_items.extend(items.iter().cloned());
    }
}
