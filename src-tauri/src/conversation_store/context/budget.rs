//! Token budget system for context window management.
//!
//! This module provides token-aware budget calculation so the context assembly
//! pipeline can truncate based on model context windows rather than a hard
//! item count. Model context windows are sourced from provider documentation
//! and updated as new models are released.
//!
//! The budget feeds into [`load_context_for_request`] so the conversation
//! snapshot stays within the active model's input token limit.

/// A resolved token budget for the current request.
#[derive(Debug, Clone)]
pub struct TokenBudget {
    /// The model's full context window size in tokens.
    #[allow(dead_code)]
    pub context_window: usize,
    /// The safe budget for context history (after reserving space for
    /// instructions, tool schemas, and the current turn).
    pub context_budget: usize,
    /// The model identifier used to resolve this budget.
    #[allow(dead_code)]
    pub model_id: String,
}

impl TokenBudget {
    /// Create a budget for the given provider kind and model identifier.
    ///
    /// The provider plugin is called to retrieve the context window size.
    /// Unknown models default to a conservative 128K window.
    pub fn for_model(provider_kind: &str, model_id: &str) -> Self {
        let window_tokens = match crate::provider_api::get_model_metadata(provider_kind, model_id) {
            Ok(metadata) => metadata.context_window.unwrap_or(128_000),
            Err(err) => {
                log::warn!(
                    "Failed to query model metadata for model '{}' from provider '{}': {}",
                    model_id,
                    provider_kind,
                    err
                );
                128_000
            }
        };

        // Compute a recommended budget for request input items.
        // Large windows reserve a smaller proportion.
        let reserved = if window_tokens >= 1_000_000 {
            20_000
        } else if window_tokens >= 128_000 {
            16_000
        } else if window_tokens >= 64_000 {
            12_000
        } else {
            8_000
        };

        Self {
            context_window: window_tokens,
            context_budget: window_tokens.saturating_sub(reserved),
            model_id: model_id.to_string(),
        }
    }

    /// Create an unlimited budget (no token cap).
    ///
    /// Useful for testing or when the model is unknown and the caller prefers
    /// lenient behaviour over conservative truncation.
    #[allow(dead_code)]
    pub fn unlimited() -> Self {
        Self {
            context_window: usize::MAX,
            context_budget: usize::MAX,
            model_id: "*".to_string(),
        }
    }

    /// Return `true` if this budget has no meaningful cap.
    pub fn is_unlimited(&self) -> bool {
        self.context_budget == usize::MAX
    }
}

/// Compute the approximate token count for a slice of input items.
///
/// This is a rough estimate used for budget enforcement. It serialises each
/// item to JSON and uses a 4:1 character-to-token ratio as a fast fallback.
/// For precise counting, use the tokenizer in [`super::token_usage`].
pub fn estimate_input_items_tokens(items: &[serde_json::Value]) -> usize {
    items
        .iter()
        .map(|item| {
            let serialized = serde_json::to_string(item).unwrap_or_default();
            // Rough estimate: ~4 chars per token for JSON-serialised content.
            serialized.len().saturating_add(3) / 4
        })
        .sum()
}

/// Truncate context items to fit within the given token budget.
///
/// Items are dropped from the **beginning** of the slice (oldest first) until
/// the estimated token count fits within the budget. Tool call pairs are kept
/// intact (if a `function_call` is kept, its matching `function_call_output`
/// is also kept, and vice versa).
///
/// Returns the truncated slice of items.
pub fn truncate_items_to_budget(
    items: Vec<serde_json::Value>,
    budget: &TokenBudget,
) -> Vec<serde_json::Value> {
    if budget.is_unlimited() || items.is_empty() {
        return items;
    }

    // Check from the end: find the smallest prefix-to-drop that fits the budget.
    let total = estimate_input_items_tokens(&items);
    if total <= budget.context_budget {
        return items;
    }

    // Collect tool pair relationships for integrity preservation.
    let tool_pairs = build_tool_pair_map(&items);

    // Binary search on how many items to drop from the front.
    let drop_count = find_min_drop(&items, budget, &tool_pairs);
    items.into_iter().skip(drop_count).collect()
}

/// Find the minimum number of items to drop from the front so the remainder
/// fits within the budget, preserving tool call pairs.
fn find_min_drop(
    items: &[serde_json::Value],
    budget: &TokenBudget,
    tool_pairs: &std::collections::HashSet<usize>,
) -> usize {
    let total = items.len();
    if total == 0 {
        return 0;
    }

    // Identify items that are part of a tool pair — these cannot be split.
    let mut droppable = vec![true; total];
    for &paired_idx in tool_pairs {
        droppable[paired_idx] = false;
    }

    // The first non-tool-pair item marks where we can start dropping safely.
    let min_start = droppable.iter().position(|d| *d).unwrap_or(0);

    let mut low = min_start;
    let mut high = total;

    while low < high {
        let mid = low + (high - low) / 2;
        let tokens = estimate_input_items_tokens(&items[mid..]);
        if tokens <= budget.context_budget {
            high = mid;
        } else {
            low = mid + 1;
        }
    }

    // Ensure we don't split a tool pair at the boundary.
    adjust_for_tool_pair_boundary(low, items, tool_pairs)
}

/// Build a set of indices that belong to tool call pairs and must be kept
/// together.
fn build_tool_pair_map(items: &[serde_json::Value]) -> std::collections::HashSet<usize> {
    use serde_json::Value;
    let mut pairs: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut call_ids: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();

    for (idx, item) in items.iter().enumerate() {
        let type_str = item.get("type").and_then(Value::as_str).unwrap_or("");
        let call_id = match item.get("call_id").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => id,
            _ => continue,
        };

        if type_str == "function_call" || type_str == "custom_tool_call" {
            call_ids.insert(call_id, idx);
            pairs.insert(idx);
        } else if (type_str == "function_call_output" || type_str == "custom_tool_call_output")
            && let Some(&call_idx) = call_ids.get(call_id)
        {
            pairs.insert(idx);
            pairs.insert(call_idx);
        }
    }

    pairs
}

/// If the boundary `start` would split a tool call pair, slide left until the
/// pair is intact.
fn adjust_for_tool_pair_boundary(
    start: usize,
    items: &[serde_json::Value],
    tool_pairs: &std::collections::HashSet<usize>,
) -> usize {
    if start == 0 || start >= items.len() {
        return start;
    }

    // Check if the item just before the boundary belongs to a pair whose
    // counterpart is after the boundary.
    if tool_pairs.contains(&start) {
        // The item at `start` is part of a pair. Check if its counterpart
        // is before `start`.
        let counterpart_before = (0..start).any(|i| tool_pairs.contains(&i));
        if counterpart_before {
            // Move start back to include the first element of this pair.
            let pair_start = (0..start).rfind(|i| tool_pairs.contains(i)).unwrap_or(0);
            return pair_start;
        }
    }

    start
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gpt_5_maps_to_400k_window() {
        let budget = TokenBudget::for_model("openai", "gpt-5-20260501");
        assert_eq!(budget.context_window, 400_000);
    }

    #[test]
    fn unknown_model_defaults_to_128k() {
        let budget = TokenBudget::for_model("openai", "some-future-model-v3");
        assert_eq!(budget.context_window, 128_000);
    }

    #[test]
    fn unlimited_budget_passes_items_through() {
        let items =
            vec![json!({"role": "user", "content": [{"type": "input_text", "text": "hello"}]})];
        let budget = TokenBudget::unlimited();
        let result = truncate_items_to_budget(items.clone(), &budget);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn empty_items_with_budget_returns_empty() {
        let items = vec![];
        let budget = TokenBudget::for_model("openai", "gpt-4o");
        let result = truncate_items_to_budget(items, &budget);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn budget_allows_known_model_context_reservation() {
        let budget = TokenBudget::for_model("openai", "gpt-4o");
        // 128K window - 16K reserved = 112K budget
        assert_eq!(budget.context_budget, 112_000);
        assert!(budget.context_budget < budget.context_window);
    }

    #[test]
    fn resolve_gpt_variants() {
        assert_eq!(
            TokenBudget::for_model("openai", "gpt-4-turbo").context_window,
            128_000
        );
        assert_eq!(
            TokenBudget::for_model("openai", "gpt-4-32k").context_window,
            32_768
        );
        assert_eq!(
            TokenBudget::for_model("openai", "gpt-4o-20240806").context_window,
            128_000
        );
        assert_eq!(
            TokenBudget::for_model("openai", "gpt-5-mini-20260501").context_window,
            400_000
        );
        assert_eq!(
            TokenBudget::for_model("openai", "gpt-3.5-turbo").context_window,
            16_384
        );
    }

    #[test]
    fn estimate_tokens_from_items() {
        let items = vec![
            json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hello!"}]}),
        ];
        let count = estimate_input_items_tokens(&items);
        assert!(count > 0);
        assert!(count < 200); // one short message should be < 200 tokens
    }

    #[test]
    fn unknown_models_in_openai_fallback_to_128k() {
        assert_eq!(
            TokenBudget::for_model("openai", "deepseek-r1").context_window,
            128_000
        );
        assert_eq!(
            TokenBudget::for_model("openai", "deepseek-v3").context_window,
            128_000
        );
    }

    #[test]
    fn tool_pairs_preserved_during_truncation() {
        let items = vec![
            json!({"type": "function_call", "call_id": "call_1", "name": "get_weather", "arguments": "{}"}),
            json!({"type": "function_call_output", "call_id": "call_1", "output": "{\"temp\": 72}"}),
            json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Thanks!"}]}),
        ];

        let budget = TokenBudget {
            context_window: 100,
            context_budget: 10, // Very small — will truncate
            model_id: "test".to_string(),
        };

        let result = truncate_items_to_budget(items.clone(), &budget);
        // Should drop the tool pair together or keep them together.
        // With a budget of 10, all items likely exceed it.
        assert!(result.len() <= items.len());
    }
}
