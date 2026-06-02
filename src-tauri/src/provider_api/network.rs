//! Basic host networking primitives for provider plugins.
//!
//! Provider plugins own endpoint selection, headers, payload shape, and stream
//! parsing. This module intentionally stays protocol-generic: it applies HTTP
//! headers and query parameters and splits Server-Sent Events frames without
//! knowing which model vendor produced them.

use std::collections::BTreeMap;

use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{RequestBuilder};

pub fn apply_headers_to_reqwest(
    mut builder: RequestBuilder,
    headers: &BTreeMap<String, String>,
) -> Result<RequestBuilder, String> {
    for (key, value) in headers {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|err| format!("Invalid HTTP header name '{key}': {err}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|err| format!("Invalid HTTP header value for '{key}': {err}"))?;
        builder = builder.header(header_name, header_value);
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

