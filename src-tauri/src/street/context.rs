//! Street context injection — formats Street notifications as system messages.

use serde_json::{Value, json};

/// Format a list of Street items into a compact text summary for context injection.
pub fn format_street_items(items: &[crate::street::types::StreetItem]) -> String {
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let payload_str = serde_json::to_string(&item.payload).unwrap_or_default();
            let truncated: String = payload_str.chars().take(200).collect();
            let suffix = if payload_str.len() > 200 { "..." } else { "" };
            format!(
                "({}) [{}] [{}] {}: {}{}",
                i + 1,
                item.priority.as_str(),
                item.source.as_str(),
                item.title,
                truncated,
                suffix,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build a system message containing Street notifications.
///
/// This is injected into the hop prefix at the start of each turn,
/// so the model sees pending async results without needing to poll.
pub fn build_street_context_system_item(count: usize, formatted: &str) -> Value {
    json!({
        "role": "system",
        "content": [{
            "type": "input_text",
            "text": format!(
                "[Street] {} pending notification(s) since your last turn:\n\n{}\n\n\
                 These are results from async work (sub-agents, background tools) \
                 that you previously started. Review them and take action if needed. \
                 You do NOT need to poll for status — results are automatically \
                 delivered here when work completes.",
                count, formatted
            ),
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::street::types::{Priority, StreetItem, StreetSource};
    use serde_json::json;

    #[test]
    fn test_format_street_items() {
        let items = vec![
            StreetItem::new(
                "conv-1",
                StreetSource::SubAgent,
                Priority::Normal,
                "Sub-agent completed",
                json!({"files": 5}),
            ),
            StreetItem::new(
                "conv-1",
                StreetSource::BackgroundJob,
                Priority::Low,
                "BG job done",
                json!({"status": "ok"}),
            ),
        ];
        let formatted = format_street_items(&items);
        assert!(formatted.contains("Sub-agent completed"));
        assert!(formatted.contains("BG job done"));
        assert!(formatted.contains("[normal]"));
        assert!(formatted.contains("[low]"));
    }

    #[test]
    fn test_build_context_item() {
        let item = build_street_context_system_item(3, "(1) test\n(2) test2\n(3) test3");
        let text = item["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("[Street]"));
        assert!(text.contains("3 pending notification"));
        assert!(text.contains("(1) test"));
    }
}
