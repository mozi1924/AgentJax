/// Split one complete Server-Sent Events block from a mutable text buffer.
///
/// Providers disagree on CRLF vs LF line endings, so this helper accepts both.
/// The returned `rest` should be written back to the caller's buffer.
pub(crate) fn split_sse_event_block(buffer: &str) -> Option<(String, String)> {
    if let Some(pos) = buffer.find("\r\n\r\n") {
        let block = buffer[..pos].to_string();
        let rest = buffer[pos + 4..].to_string();
        return Some((block, rest));
    }
    if let Some(pos) = buffer.find("\n\n") {
        let block = buffer[..pos].to_string();
        let rest = buffer[pos + 2..].to_string();
        return Some((block, rest));
    }
    None
}

/// Extract the concatenated `data:` payload from an SSE event block.
pub(crate) fn sse_data_payload(block: &str) -> Option<String> {
    let mut data_lines = Vec::new();

    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}
