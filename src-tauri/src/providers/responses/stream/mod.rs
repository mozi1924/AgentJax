mod parser;
mod payload;
mod transport;

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
    let use_sse = resolved.provider.stream_transport == "sse";

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
        log::warn!(
            "WebSocket transport failed for provider '{}', retrying with SSE transport",
            resolved.provider_key
        );
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
            log::warn!(
                "WebSocket store=false retry failed for provider '{}', retrying with SSE transport",
                resolved.provider_key
            );
            return transport::create_response_streaming_sse(
                resolved, req, behavior, false, cancel_rx, on_delta,
            )
            .await;
        }

        return store_false_attempt;
    }

    if should_retry_without_previous_response(&first_attempt, req.previous_response_id.as_deref()) {
        let mut retry_req = req.clone();
        retry_req.previous_response_id = None;

        let retry_attempt = if use_sse {
            transport::create_response_streaming_sse(
                resolved,
                &retry_req,
                behavior,
                persistence,
                cancel_rx,
                on_delta,
            )
            .await
        } else {
            transport::create_response_streaming_websocket(
                resolved,
                &retry_req,
                behavior,
                persistence,
                cancel_rx,
                on_delta,
            )
            .await
        };

        if !use_sse && retry_attempt.is_err() {
            log::warn!(
                "WebSocket previous_response retry failed for provider '{}', retrying with SSE transport",
                resolved.provider_key
            );
            return transport::create_response_streaming_sse(
                resolved,
                &retry_req,
                behavior,
                persistence,
                cancel_rx,
                on_delta,
            )
            .await;
        }

        return retry_attempt;
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

fn should_retry_without_previous_response(
    result: &Result<ResponseStreamResult, String>,
    previous_response_id: Option<&str>,
) -> bool {
    if previous_response_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return false;
    }

    let Err(err) = result else {
        return false;
    };

    err.contains("previous_response_not_found")
        || err.contains("Previous response with id")
        || err.contains("\"param\":\"previous_response_id\"")
}
