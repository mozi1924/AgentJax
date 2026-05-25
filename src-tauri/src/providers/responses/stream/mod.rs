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
    pub force_store_false: bool,
    pub retry_store_false: bool,
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
    let persistence = if behavior.force_store_false {
        false
    } else {
        resolved.provider.store_responses
    };
    let websocket_key = websocket_fallback_key(resolved);
    let use_sse =
        resolved.provider.stream_transport == "sse" || websocket_is_downgraded(&websocket_key);

    let first_attempt = if use_sse {
        transport::create_response_streaming_sse(
            resolved,
            req,
            behavior,
            persistence,
            cancel_rx,
            on_delta,
        )
        .await
    } else {
        transport::create_response_streaming_websocket(
            resolved,
            req,
            behavior,
            persistence,
            cancel_rx,
            on_delta,
        )
        .await
    };

    if !use_sse && first_attempt.is_err() {
        if let Err(err) = &first_attempt {
            log_websocket_fallback(resolved, &websocket_key, "WebSocket transport failed", err);
        }
        return transport::create_response_streaming_sse(
            resolved,
            req,
            behavior,
            persistence,
            cancel_rx,
            on_delta,
        )
        .await;
    }

    if behavior.retry_store_false && should_retry_with_store_false(&first_attempt, persistence) {
        let store_false_attempt = if use_sse {
            transport::create_response_streaming_sse(
                resolved, req, behavior, false, cancel_rx, on_delta,
            )
            .await
        } else {
            transport::create_response_streaming_websocket(
                resolved, req, behavior, false, cancel_rx, on_delta,
            )
            .await
        };

        if !use_sse && store_false_attempt.is_err() {
            if let Err(err) = &store_false_attempt {
                log_websocket_fallback(
                    resolved,
                    &websocket_key,
                    "WebSocket store=false retry failed",
                    err,
                );
            }
            return transport::create_response_streaming_sse(
                resolved, req, behavior, false, cancel_rx, on_delta,
            )
            .await;
        }

        return store_false_attempt;
    }

    first_attempt
}

fn should_retry_with_store_false(
    result: &Result<ResponseStreamResult, String>,
    store_value: bool,
) -> bool {
    if !store_value {
        return false;
    }

    let Err(err) = result else {
        return false;
    };

    err.contains("Store must be set to false")
        || err.contains("store must be set to false")
        || err.contains("\"Store must be set to false\"")
}
