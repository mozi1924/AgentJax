//! Shared HTTP primitives for MCP transport and provider API calls.
//!
//! Both subsystems speak HTTP under the hood — MCP uses Streamable HTTP,
//! provider APIs use REST / SSE — and share the same header validation
//! logic.  This module keeps that logic in one place so the two don't
//! drift apart.

use std::collections::{BTreeMap, HashMap};

use reqwest::header::{HeaderName, HeaderValue};

use crate::error::{AgentJaxError, AgentJaxResult};

/// Parse a string-to-string header map into validated `reqwest` header pairs.
///
/// Keys and values are trimmed; empty entries are silently skipped.
/// Returns `AgentJaxError::config` on invalid header names or values so
/// the caller (whether MCP or provider config) gets a consistent error.
pub fn parse_headers_map(
    headers: &BTreeMap<String, String>,
) -> AgentJaxResult<HashMap<HeaderName, HeaderValue>> {
    let mut parsed = HashMap::new();
    for (name, value) in headers {
        let key = name.trim();
        let val = value.trim();
        if key.is_empty() || val.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(key.as_bytes()).map_err(|e| {
            AgentJaxError::config(format!("Invalid HTTP header name '{key}': {e}"))
                .with_error_source(&e)
        })?;
        let header_value = HeaderValue::from_str(val).map_err(|e| {
            AgentJaxError::config(format!("Invalid HTTP header value for '{key}': {e}"))
                .with_error_source(&e)
        })?;
        parsed.insert(header_name, header_value);
    }
    Ok(parsed)
}
