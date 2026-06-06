//! Basic host networking primitives for provider plugins.
//!
//! Provider plugins own endpoint selection, headers, payload shape, and stream
//! parsing. This module intentionally stays protocol-generic: it splits
//! Server-Sent Events frames without knowing which model vendor produced them.

use std::collections::BTreeMap;

use reqwest::RequestBuilder;

/// Apply a validated header map to a `reqwest` request builder.
///
/// Header parsing is delegated to the shared [`crate::http_util::parse_headers_map`]
/// so that MCP transport and provider API calls share the same validation logic.
pub fn apply_headers_to_reqwest(
    mut builder: RequestBuilder,
    headers: &BTreeMap<String, String>,
) -> crate::error::AgentJaxResult<RequestBuilder> {
    let parsed = crate::http_util::parse_headers_map(headers)?;
    for (name, value) in parsed {
        builder = builder.header(name, value);
    }
    Ok(builder)
}

/// Split one complete Server-Sent Events block from a mutable text buffer.
///
/// SSE producers vary between LF and CRLF framing; both are accepted.
pub fn split_sse_event_block(buffer: &str) -> Option<(String, String)> {
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
