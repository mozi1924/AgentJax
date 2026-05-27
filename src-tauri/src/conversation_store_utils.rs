use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn normalize_title_source(value: &str) -> String {
    match value.trim().to_lowercase().as_str() {
        "manual" => "manual".to_string(),
        "auto" => "auto".to_string(),
        _ => "pending".to_string(),
    }
}

pub(crate) fn normalize_title(raw: &str) -> String {
    const DEFAULT_CONVERSATION_TITLE: &str = "新对话";
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        DEFAULT_CONVERSATION_TITLE.to_string()
    } else if cleaned.chars().count() <= 32 {
        cleaned
    } else {
        cleaned.chars().take(32).collect()
    }
}

pub(crate) fn sanitize_conversation_id(conversation_id: &str) -> String {
    let trimmed = conversation_id.trim();
    let safe = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();

    if safe.is_empty() {
        "conversation".to_string()
    } else {
        safe
    }
}

pub(crate) fn compact_preview(raw: &str) -> String {
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= 60 {
        cleaned
    } else {
        format!("{}...", cleaned.chars().take(57).collect::<String>())
    }
}

pub(crate) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn today_utc_yyyy_mm_dd() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_days = duration.as_secs() / 86_400;
    let (year, month, day) = civil_from_days(total_days as i64);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_unix_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m as u32, d as u32)
}
