use crate::message_phase::AssistantPhase;
use serde_json::Value;

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn strip_commentary_prefixes(final_text: &str, commentary_history: &[String]) -> String {
    if commentary_history.is_empty() {
        return final_text.trim().to_string();
    }

    let commentary_norms: Vec<String> = commentary_history
        .iter()
        .map(|text| normalize_whitespace(text))
        .filter(|text| !text.is_empty())
        .collect();
    if commentary_norms.is_empty() {
        return final_text.trim().to_string();
    }

    let mut remaining_lines: Vec<&str> = final_text.lines().collect();
    loop {
        let first_non_empty_idx = remaining_lines
            .iter()
            .position(|line| !line.trim().is_empty());
        let Some(idx) = first_non_empty_idx else {
            return final_text.trim().to_string();
        };
        let first_line = remaining_lines[idx].trim();
        let first_line_norm = normalize_whitespace(first_line);
        if commentary_norms.iter().any(|item| item == &first_line_norm) {
            remaining_lines.drain(..=idx);
            continue;
        }
        break;
    }

    remaining_lines.join("\n").trim().to_string()
}

pub(super) fn extract_assistant_messages_from_items(
    items: &[Value],
) -> Vec<(String, Option<AssistantPhase>)> {
    items
        .iter()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
        .filter_map(|item| {
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            Some((
                text,
                item.get("phase")
                    .and_then(Value::as_str)
                    .and_then(AssistantPhase::from_api_value),
            ))
        })
        .collect()
}

pub(super) fn resolve_hop_phase(
    explicit_phase: Option<AssistantPhase>,
    is_final_hop: bool,
) -> Option<AssistantPhase> {
    explicit_phase.or(Some(if is_final_hop {
        AssistantPhase::FinalAnswer
    } else {
        AssistantPhase::Commentary
    }))
}

pub(super) fn select_final_output_text(
    hop_messages: &[(String, Option<AssistantPhase>)],
    fallback_output_text: &str,
    commentary_history: &[String],
) -> String {
    let preferred = hop_messages
        .iter()
        .rev()
        .find(|(_, phase)| *phase != Some(AssistantPhase::Commentary))
        .map(|(text, _)| text.as_str())
        .unwrap_or(fallback_output_text);

    strip_commentary_prefixes(preferred, commentary_history)
}
