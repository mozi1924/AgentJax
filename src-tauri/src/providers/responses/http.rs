use std::collections::BTreeMap;

use reqwest::header::{HeaderName, HeaderValue};
use reqwest::{RequestBuilder, Url};
use tokio_tungstenite::tungstenite::http::Request;

use crate::config::ProviderConfig;

fn has_header_case_insensitive(headers: &BTreeMap<String, String>, name: &str) -> bool {
    headers.keys().any(|key| key.eq_ignore_ascii_case(name))
}

pub(crate) fn merge_request_headers(
    base_headers: &[(&str, &str)],
    provider: &ProviderConfig,
    runtime_headers: Option<&BTreeMap<String, String>>,
    credential: Option<&str>,
) -> BTreeMap<String, String> {
    let mut merged = BTreeMap::new();

    for (key, value) in base_headers {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        merged.insert(key.to_string(), value.to_string());
    }

    for (key, value) in provider.resolved_http_headers() {
        if !key.trim().is_empty() && !value.trim().is_empty() {
            merged.insert(key, value);
        }
    }

    if let Some(runtime_headers) = runtime_headers {
        for (key, value) in runtime_headers {
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                continue;
            }
            merged.insert(key.to_string(), value.to_string());
        }
    }

    if !has_header_case_insensitive(&merged, "Authorization") {
        if let Some(credential) = credential.map(str::trim).filter(|value| !value.is_empty()) {
            merged.insert("Authorization".to_string(), format!("Bearer {credential}"));
        }
    }

    merged
}

pub(crate) fn apply_query_params_to_url(
    url: &str,
    query_params: &BTreeMap<String, String>,
) -> Result<String, String> {
    if query_params.is_empty() {
        return Ok(url.to_string());
    }

    let mut parsed = Url::parse(url).map_err(|e| format!("Failed to parse URL '{url}': {e}"))?;
    {
        let mut pairs = parsed.query_pairs_mut();
        for (key, value) in query_params {
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                continue;
            }
            pairs.append_pair(key, value);
        }
    }

    Ok(parsed.to_string())
}

pub(crate) fn apply_headers_to_reqwest(
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
            .map_err(|e| format!("Invalid HTTP header name '{key}': {e}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| format!("Invalid HTTP header value for '{key}': {e}"))?;
        builder = builder.header(header_name, header_value);
    }

    Ok(builder)
}

pub(crate) fn apply_headers_to_websocket_request(
    request: &mut Request<()>,
    headers: &BTreeMap<String, String>,
) -> Result<(), String> {
    for (key, value) in headers {
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }

        let header_name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|e| format!("Invalid WebSocket header name '{key}': {e}"))?;
        let header_value = HeaderValue::from_str(value)
            .map_err(|e| format!("Invalid WebSocket header value for '{key}': {e}"))?;
        request.headers_mut().insert(header_name, header_value);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply_query_params_to_url, merge_request_headers};
    use crate::config::ProviderConfig;

    #[test]
    fn merge_request_headers_preserves_layer_order_and_runtime_override() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut provider = ProviderConfig::default();

        let mut http_headers = serde_json::Map::new();
        http_headers.insert(
            "X-Test".to_string(),
            serde_json::Value::String("provider".to_string()),
        );
        http_headers.insert(
            "Authorization".to_string(),
            serde_json::Value::String("ApiKey provider-auth".to_string()),
        );
        provider.custom_settings.insert(
            "httpHeaders".to_string(),
            serde_json::Value::Object(http_headers),
        );

        let mut env_http_headers = serde_json::Map::new();
        env_http_headers.insert(
            "X-Test".to_string(),
            serde_json::Value::String("TEST_X_HEADER".to_string()),
        );
        provider.custom_settings.insert(
            "envHttpHeaders".to_string(),
            serde_json::Value::Object(env_http_headers),
        );

        unsafe {
            std::env::set_var("TEST_X_HEADER", "env");
        }

        let mut runtime = std::collections::BTreeMap::new();
        runtime.insert("X-Test".to_string(), "runtime".to_string());

        let headers = merge_request_headers(
            &[("Content-Type", "application/json")],
            &provider,
            Some(&runtime),
            Some("credential-token"),
        );

        assert_eq!(
            headers.get("Content-Type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(headers.get("X-Test").map(String::as_str), Some("runtime"));
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("ApiKey provider-auth")
        );

        unsafe {
            std::env::remove_var("TEST_X_HEADER");
        }
    }

    #[test]
    fn merge_request_headers_adds_bearer_when_missing_authorization() {
        let provider = ProviderConfig::default();
        let headers = merge_request_headers(
            &[("Accept", "application/json")],
            &provider,
            None,
            Some("token-123"),
        );

        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer token-123")
        );
    }

    #[test]
    fn apply_query_params_to_url_keeps_existing_query() {
        let mut query = std::collections::BTreeMap::new();
        query.insert("api-version".to_string(), "2026-05-01".to_string());
        query.insert("project".to_string(), "alpha".to_string());

        let updated = apply_query_params_to_url("https://example.com/v1/models?x=1", &query)
            .expect("query params should be appended");

        assert!(updated.contains("x=1"));
        assert!(updated.contains("api-version=2026-05-01"));
        assert!(updated.contains("project=alpha"));
    }
}
