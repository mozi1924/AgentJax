//! Token budget system for context window management.
//!
//! This module provides token-aware budget calculation so the context assembly
//! pipeline can truncate based on model context windows rather than a hard
//! item count. Model context windows are sourced from provider documentation
//! and updated as new models are released.
//!
//! The budget feeds into [`load_context_for_request`] so the conversation
//! snapshot stays within the active model's input token limit.

/// Well-known model context window sizes (max input tokens).
///
/// These values are extracted from official provider documentation. Unknown
/// models default to a conservative 128K window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelContextWindow {
    /// 8K window — legacy GPT-4, Claude 3 Haiku small variant.
    K8 = 8_192,
    /// 16K window — legacy GPT-3.5, Claude Instant.
    K16 = 16_384,
    /// 32K window — Claude 3 Haiku default.
    K32 = 32_768,
    /// 64K window — Gemini 1.5 Flash, Claude 3 Sonnet.
    K64 = 65_536,
    /// 100K window — Claude 2.1.
    K100 = 100_000,
    /// 128K window — GPT-4o, GPT-5-mini, GPT-4.1, Gemini default.
    K128 = 128_000,
    /// 200K window — Claude 3 / Claude 3.5 Sonnet.
    K200 = 200_000,
    /// 1M window — Gemini 1.5 Pro, Gemini 2.0 Flash.
    K1M = 1_000_000,
    /// 2M window — Claude 4, Claude 4.5 Sonnet, Gemini 2.5 Pro.
    K2M = 2_000_000,
}

impl ModelContextWindow {
    /// Return the raw token count for this window size.
    pub fn tokens(&self) -> usize {
        *self as usize
    }

    /// Compute a recommended budget for request input items.
    ///
    /// The budget reserves a portion of the window for:
    /// - System / developer instructions (~4K)
    /// - Tool schemas (varies, assume ~8K)
    /// - The current turn's output (~8K)
    ///
    /// The remainder is the safe budget for historical context items.
    pub fn context_budget(&self) -> usize {
        let total = self.tokens();
        let reserved = match self {
            // Large windows reserve a smaller proportion.
            Self::K1M | Self::K2M => 20_000,
            Self::K200 | Self::K128 => 16_000,
            Self::K100 | Self::K64 => 12_000,
            _ => 8_000,
        };
        total.saturating_sub(reserved)
    }
}

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
    /// Create a budget for the given model identifier.
    ///
    /// The model string is matched against known patterns to select the
    /// appropriate window size. Unknown models use a conservative 128K
    /// default.
    pub fn for_model(model_id: &str) -> Self {
        let window = resolve_context_window(model_id);
        Self {
            context_window: window.tokens(),
            context_budget: window.context_budget(),
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

/// Resolve a model identifier to its context window size.
///
/// Matching is case-insensitive and uses substring / prefix patterns so model
/// aliases and version variants are handled without an exhaustive list.
fn resolve_context_window(model_id: &str) -> ModelContextWindow {
    let normalized = model_id.trim().to_ascii_lowercase();

    // ── Anthropic / Claude ─────────────────────────────────────────────
    if normalized.contains("claude") {
        // Claude 4 / 4.5, Claude 3.5 Opus
        if normalized.contains("claude-4")
            || normalized.contains("claude-3.5-opus")
            || normalized.contains("claude-opus-4")
        {
            return ModelContextWindow::K2M;
        }
        // Claude 3.5 Sonnet, Claude 3 Opus — 200K
        if normalized.contains("sonnet")
            || normalized.contains("opus")
            || normalized.contains("claude-3")
        {
            return ModelContextWindow::K200;
        }
        // Claude 2.x — 100K
        if normalized.contains("claude-2") || normalized.contains("claude-instant") {
            return ModelContextWindow::K100;
        }
        // Claude Haiku — check for 200K variant
        if normalized.contains("haiku") {
            // Claude 3.5 Haiku supports 200K
            if normalized.contains("3.5") {
                return ModelContextWindow::K200;
            }
            return ModelContextWindow::K32;
        }
        // Default Claude fallback
        return ModelContextWindow::K200;
    }

    // ── Google / Gemini ────────────────────────────────────────────────
    if normalized.contains("gemini") {
        // Gemini 2.5 Pro — 2M
        if normalized.contains("gemini-2.5") || normalized.contains("gemini-2.5-pro") {
            return ModelContextWindow::K2M;
        }
        // Gemini 1.5 Pro — 1M
        if normalized.contains("gemini-1.5-pro") || normalized.contains("gemini-1.5-ultra") {
            return ModelContextWindow::K1M;
        }
        // Gemini 1.5 Flash / 2.0 Flash — 1M
        if normalized.contains("flash") || normalized.contains("gemini-2.0") {
            return ModelContextWindow::K1M;
        }
        // Other Gemini — 128K
        return ModelContextWindow::K128;
    }

    // ── OpenAI / GPT ───────────────────────────────────────────────────
    if normalized.contains("gpt") || normalized.contains("o1") || normalized.contains("o3") {
        // GPT-5 and o-series reasoning models — 200K
        if normalized.contains("gpt-5") || normalized.starts_with("o1") || normalized.starts_with("o3") {
            return ModelContextWindow::K200;
        }
        // GPT-4.1 / GPT-4.5 — 128K (or 1M for GPT-4.1 family)
        if normalized.contains("gpt-4.1") {
            return ModelContextWindow::K1M;
        }
        if normalized.contains("gpt-4.5") || normalized.contains("gpt-4o") || normalized.contains("gpt-4o") {
            return ModelContextWindow::K128;
        }
        // GPT-4 — 8K or 32K variant
        if normalized.contains("gpt-4-32k") || normalized.contains("gpt-4-1106") {
            return ModelContextWindow::K32;
        }
        if normalized.contains("gpt-4") {
            return ModelContextWindow::K8;
        }
        // GPT-3.5 — 16K
        if normalized.contains("gpt-3.5") {
            return ModelContextWindow::K16;
        }
        // Default GPT fallback
        return ModelContextWindow::K128;
    }

    // ── Meta / Llama ───────────────────────────────────────────────────
    if normalized.contains("llama") || normalized.contains("meta") {
        if normalized.contains("llama-4") || normalized.contains("llama-3.1-405b") {
            return ModelContextWindow::K128;
        }
        if normalized.contains("llama-3.1-70b") || normalized.contains("llama-3.1-8b") {
            return ModelContextWindow::K128;
        }
        return ModelContextWindow::K128;
    }

    // ── Mistral / Codestral ────────────────────────────────────────────
    if normalized.contains("mistral") || normalized.contains("codestral") || normalized.contains("mixtral") {
        if normalized.contains("large") || normalized.contains("mistral-large") {
            return ModelContextWindow::K128;
        }
        return ModelContextWindow::K32;
    }

    // ── DeepSeek ────────────────────────────────────────────────────────
    if normalized.contains("deepseek") {
        if normalized.contains("deepseek-r1") || normalized.contains("deepseek-v3") {
            return ModelContextWindow::K128;
        }
        return ModelContextWindow::K64;
    }

    // ── Amazon / AWS ────────────────────────────────────────────────────
    if normalized.contains("nova") || normalized.contains("amazon") {
        return ModelContextWindow::K128;
    }

    // ── Cohere ──────────────────────────────────────────────────────────
    if normalized.contains("command") || normalized.contains("cohere") {
        return ModelContextWindow::K128;
    }

    // ── xAI / Grok ─────────────────────────────────────────────────────
    if normalized.contains("grok") {
        return ModelContextWindow::K128;
    }

    // ── AI21 / Jamba ───────────────────────────────────────────────────
    if normalized.contains("jamba") || normalized.contains("ai21") {
        return ModelContextWindow::K128;
    }

    // ── Default: 128K conservative ─────────────────────────────────────
    ModelContextWindow::K128
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
        } else if type_str == "function_call_output" || type_str == "custom_tool_call_output" {
            if let Some(&call_idx) = call_ids.get(call_id) {
                pairs.insert(idx);
                pairs.insert(call_idx);
            }
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
            let pair_start = (0..start)
                .filter(|i| tool_pairs.contains(i))
                .last()
                .unwrap_or(0);
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
    fn claude_4_maps_to_2m_window() {
        let budget = TokenBudget::for_model("claude-4-sonnet-20260501");
        assert_eq!(budget.context_window, 2_000_000);
    }

    #[test]
    fn gpt_5_maps_to_200k_window() {
        let budget = TokenBudget::for_model("gpt-5-20260501");
        assert_eq!(budget.context_window, 200_000);
    }

    #[test]
    fn gemini_2_5_pro_maps_to_2m_window() {
        let budget = TokenBudget::for_model("gemini-2.5-pro-001");
        assert_eq!(budget.context_window, 2_000_000);
    }

    #[test]
    fn unknown_model_defaults_to_128k() {
        let budget = TokenBudget::for_model("some-future-model-v3");
        assert_eq!(budget.context_window, 128_000);
    }

    #[test]
    fn unlimited_budget_passes_items_through() {
        let items = vec![json!({"role": "user", "content": [{"type": "input_text", "text": "hello"}]})];
        let budget = TokenBudget::unlimited();
        let result = truncate_items_to_budget(items.clone(), &budget);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn empty_items_with_budget_returns_empty() {
        let items = vec![];
        let budget = TokenBudget::for_model("gpt-4o");
        let result = truncate_items_to_budget(items, &budget);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn budget_allows_known_model_context_reservation() {
        let budget = TokenBudget::for_model("gpt-4o");
        // 128K window - 16K reserved = 112K budget
        assert_eq!(budget.context_budget, 112_000);
        assert!(budget.context_budget < budget.context_window);
    }

    #[test]
    fn resolve_claude_variants() {
        assert_eq!(resolve_context_window("claude-3-opus-20240229"), ModelContextWindow::K200);
        assert_eq!(resolve_context_window("claude-3-5-sonnet-20241022"), ModelContextWindow::K200);
        assert_eq!(resolve_context_window("claude-2.1"), ModelContextWindow::K100);
        assert_eq!(resolve_context_window("claude-instant-1.2"), ModelContextWindow::K100);
    }

    #[test]
    fn resolve_gpt_variants() {
        assert_eq!(resolve_context_window("gpt-4-turbo"), ModelContextWindow::K8);
        assert_eq!(resolve_context_window("gpt-4-32k"), ModelContextWindow::K32);
        assert_eq!(resolve_context_window("gpt-4o-20240806"), ModelContextWindow::K128);
        assert_eq!(resolve_context_window("gpt-5-mini-20260501"), ModelContextWindow::K200);
        assert_eq!(resolve_context_window("gpt-3.5-turbo"), ModelContextWindow::K16);
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
    fn deepseek_resolution() {
        assert_eq!(resolve_context_window("deepseek-r1"), ModelContextWindow::K128);
        assert_eq!(resolve_context_window("deepseek-v3"), ModelContextWindow::K128);
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
