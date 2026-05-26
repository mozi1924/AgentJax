mod parser;
mod payload;
mod transport;

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use tokio::sync::watch;

use crate::config::ResolvedModelConfig;
use crate::providers::capabilities::ProviderCapabilities;
use crate::providers::types::{ProviderEventSink, ResponseStreamRequest, ResponseStreamResult};

#[derive(Debug, Clone, Copy)]
pub struct ResponsesStreamBehavior {
    pub api_label: &'static str,
    pub capabilities: ProviderCapabilities,
}

static WEBSOCKET_DOWNGRADED_PROVIDERS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn websocket_fallback_key(resolved: &ResolvedModelConfig) -> String {
    format!(
        "{}::{}",
        resolved.provider_key,
        resolved.provider.resolved_realtime_endpoint()
    )
}

fn websocket_is_downgraded(key: &str) -> bool {
    WEBSOCKET_DOWNGRADED_PROVIDERS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|cached| cached.contains(key))
        .unwrap_or(false)
}

fn mark_websocket_downgraded(key: &str) -> bool {
    WEBSOCKET_DOWNGRADED_PROVIDERS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut cached| cached.insert(key.to_string()))
        .unwrap_or(false)
}

fn log_websocket_fallback(
    resolved: &ResolvedModelConfig,
    websocket_key: &str,
    context: &str,
    err: &str,
) {
    log::warn!(
        "{} for provider '{}': {}. Retrying with SSE transport",
        context,
        resolved.provider_key,
        err
    );

    if mark_websocket_downgraded(websocket_key) {
        log::info!(
            "Provider '{}' websocket transport is temporarily disabled for this app session; future turns will use SSE",
            resolved.provider_key
        );
    }
}

pub async fn stream_response_with_behavior(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    behavior: ResponsesStreamBehavior,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let max_retries = resolved.provider.stream_max_retries.unwrap_or(0);
    let mut attempt = 0u32;

    loop {
        let result =
            stream_response_attempt_with_behavior(resolved, req, behavior, cancel_rx, on_delta)
                .await;

        let should_retry = match &result {
            Ok(_) => false,
            Err(err) => attempt < max_retries && should_retry_stream_error(err),
        };
        if !should_retry {
            return result;
        }

        attempt += 1;
        if let Err(err) = &result {
            log::warn!(
                "Streaming request attempt {}/{} failed for provider '{}': {}. Retrying",
                attempt,
                max_retries,
                resolved.provider_key,
                err
            );
        }
    }
}

async fn stream_response_attempt_with_behavior(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    behavior: ResponsesStreamBehavior,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let websocket_key = websocket_fallback_key(resolved);
    let use_sse =
        resolved.provider.stream_transport == "sse" || websocket_is_downgraded(&websocket_key);

    let first_attempt = if use_sse {
        transport::create_response_streaming_sse(resolved, req, behavior, cancel_rx, on_delta).await
    } else {
        transport::create_response_streaming_websocket(resolved, req, behavior, cancel_rx, on_delta)
            .await
    };

    if !use_sse && first_attempt.is_err() {
        if let Err(err) = &first_attempt {
            log_websocket_fallback(resolved, &websocket_key, "WebSocket transport failed", err);
        }
        return transport::create_response_streaming_sse(
            resolved, req, behavior, cancel_rx, on_delta,
        )
        .await;
    }

    first_attempt
}

fn should_retry_stream_error(err: &str) -> bool {
    let text = err.to_lowercase();
    if text.contains("timed out")
        || text.contains("timeout")
        || text.contains("failed to reach")
        || text.contains("failed to connect websocket transport")
        || text.contains("websocket receive error")
        || text.contains("failed to read streaming response")
    {
        return true;
    }

    text.contains("api error (429")
        || text.contains("api error (500")
        || text.contains("api error (502")
        || text.contains("api error (503")
        || text.contains("api error (504")
}
