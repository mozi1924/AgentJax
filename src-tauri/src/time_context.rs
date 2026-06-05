use chrono::{Local, TimeZone, Utc};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Canonicalized time snapshot shared across prompt assembly and tool results.
///
/// Keeping both local and UTC renderings gives the model a stable notion of
/// "when" without forcing it to call a time tool just to orient itself.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeSnapshot {
    pub unix_ms: i64,
    pub local_rfc3339: String,
    pub utc_rfc3339: String,
    pub local_offset: String,
}

impl TimeSnapshot {
    pub fn from_unix_ms(unix_ms: i64) -> Self {
        let safe_unix_ms = unix_ms.max(0);
        let utc_dt = Utc
            .timestamp_millis_opt(safe_unix_ms)
            .single()
            .unwrap_or_else(Utc::now);
        let local_dt = Local
            .timestamp_millis_opt(safe_unix_ms)
            .single()
            .unwrap_or_else(Local::now);

        Self {
            unix_ms: safe_unix_ms,
            local_rfc3339: local_dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            utc_rfc3339: utc_dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            local_offset: local_dt.offset().to_string(),
        }
    }

    pub fn display_text(&self) -> String {
        format!(
            "{} (UTC {}, unix_ms={})",
            self.local_rfc3339, self.utc_rfc3339, self.unix_ms
        )
    }
}

pub fn render_timed_message(label: &str, unix_ms: i64, text: &str) -> String {
    let snapshot = TimeSnapshot::from_unix_ms(unix_ms);
    format!(
        "[{} at {}]\n{}",
        label,
        snapshot.display_text(),
        text.trim()
    )
}

pub fn build_temporal_context_system_item(
    request_started_at_unix_ms: i64,
    user_message_received_at_unix_ms: i64,
) -> Value {
    let request_started_at = TimeSnapshot::from_unix_ms(request_started_at_unix_ms);
    let user_message_received_at = TimeSnapshot::from_unix_ms(user_message_received_at_unix_ms);
    let payload = json!({
        "type": "agentjax_temporal_context",
        "requestStartedAt": request_started_at,
        "currentUserMessageReceivedAt": user_message_received_at,
        "guidance": [
            "Use the timestamps embedded in user and assistant messages plus tool-result _meta as the authoritative timeline for this turn.",
            "Do not call get_system_time just to learn the current time unless the user explicitly asks for a fresh time re-check."
        ]
    });

    json!({
        "role": "system",
        "content": [{
            "type": "input_text",
            "text": format!(
                "TEMPORAL_CONTEXT {}",
                serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
            )
        }]
    })
}

pub fn attach_tool_output_time_metadata(
    output: &Value,
    started_at_unix_ms: i64,
    completed_at_unix_ms: Option<i64>,
    duration_ms: Option<u64>,
) -> Value {
    let mut root = match output {
        Value::Object(map) => map.clone(),
        _ => {
            let mut wrapped = Map::new();
            wrapped.insert("result".to_string(), output.clone());
            wrapped
        }
    };

    let mut meta = root
        .get("_meta")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    meta.insert(
        "startedAt".to_string(),
        serde_json::to_value(TimeSnapshot::from_unix_ms(started_at_unix_ms)).unwrap_or(Value::Null),
    );
    if let Some(completed_at_unix_ms) = completed_at_unix_ms {
        meta.insert(
            "completedAt".to_string(),
            serde_json::to_value(TimeSnapshot::from_unix_ms(completed_at_unix_ms))
                .unwrap_or(Value::Null),
        );
    }
    if let Some(duration_ms) = duration_ms {
        meta.insert("durationMs".to_string(), Value::from(duration_ms));
    }

    root.insert("_meta".to_string(), Value::Object(meta));
    Value::Object(root)
}

#[cfg(test)]
mod tests {
    use super::{
        attach_tool_output_time_metadata, build_temporal_context_system_item,
        render_timed_message,
    };
    use serde_json::json;

    #[test]
    fn renders_timed_message_prefix() {
        let rendered = render_timed_message("User message", 1_700_000_000_000, "hello");
        assert!(rendered.contains("[User message at "));
        assert!(rendered.ends_with("hello"));
    }

    #[test]
    fn temporal_context_note_contains_marker() {
        let note = build_temporal_context_system_item(1_700_000_000_000, 1_700_000_000_123);
        let text = note["content"][0]["text"]
            .as_str()
            .expect("system note text");
        assert!(text.starts_with("TEMPORAL_CONTEXT "));
        assert!(text.contains("requestStartedAt"));
    }

    #[test]
    fn tool_output_metadata_wraps_non_object_values() {
        let wrapped = attach_tool_output_time_metadata(
            &json!("ok"),
            1_700_000_000_000,
            Some(1_700_000_000_100),
            Some(100),
        );
        assert_eq!(
            wrapped.get("result").and_then(|value| value.as_str()),
            Some("ok")
        );
        assert!(wrapped.get("_meta").is_some());
    }
}
