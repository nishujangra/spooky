use impulse_errors::ClassifiedUpstreamProxyError;

use super::*;
use crate::runtime::connection::{
    outcome::{BackendOutcomeTarget, RouteOutcomeTarget},
    request::RequestEnvelope,
    stream::{BackendFailureReason, RejectionReason, StreamPhase, TerminalReason},
};

pub(in crate::quic_listener) fn terminalize_stream(
    req: &mut RequestEnvelope,
    reason: TerminalReason,
    metrics: &Metrics,
) -> StreamPhase {
    req.transition_to_terminal_with_cleanup(reason, metrics)
}

pub(in crate::quic_listener) fn backend_failure_reason_for_proxy_error(
    err: &ProxyError,
) -> BackendFailureReason {
    match err {
        ProxyError::Timeout => BackendFailureReason::UpstreamTimeout,
        ProxyError::Tls(_) => BackendFailureReason::UpstreamTls,
        ProxyError::Transport(_) | ProxyError::Pool(_) => BackendFailureReason::UpstreamTransport,
        ProxyError::Protocol(_) => BackendFailureReason::UpstreamProtocol,
        ProxyError::Bridge(_) => BackendFailureReason::UpstreamBridge,
    }
}

pub(in crate::quic_listener) fn rejection_reason_for_status(
    status: http::StatusCode,
) -> RejectionReason {
    match status {
        http::StatusCode::PAYLOAD_TOO_LARGE => RejectionReason::RequestBodyTooLarge,
        http::StatusCode::TOO_MANY_REQUESTS => RejectionReason::RateLimited,
        http::StatusCode::SERVICE_UNAVAILABLE => RejectionReason::Overloaded,
        http::StatusCode::BAD_REQUEST => RejectionReason::ValidationFailed,
        _ => RejectionReason::ValidationFailed,
    }
}

impl QUICListener {
    pub(in crate::quic_listener) fn log_classified_upstream_failure(
        phase: &str,
        request_id: Option<u64>,
        upstream_name: Option<&str>,
        backend_addr: &str,
        classified: &ClassifiedUpstreamProxyError,
    ) {
        let request_id = request_id
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let upstream_name = upstream_name.unwrap_or("-");
        match classified.health_failure {
            Some(health_mapping) => error!(
                "phase={} request_id={} upstream={} backend={} upstream failure kind={:?} retryability={:?} health_reason={:?} metrics_reason={} detail={}",
                phase,
                request_id,
                upstream_name,
                backend_addr,
                classified.kind,
                classified.retryability,
                health_mapping.failure_reason,
                health_mapping.metrics_reason,
                classified.detail
            ),
            None => error!(
                "phase={} request_id={} upstream={} backend={} upstream failure kind={:?} retryability={:?} detail={}",
                phase,
                request_id,
                upstream_name,
                backend_addr,
                classified.kind,
                classified.retryability,
                classified.detail
            ),
        }
    }

    pub(super) fn is_internal_pool_control_error(error: &PoolError) -> bool {
        matches!(
            error,
            PoolError::InflightLimiterClosed | PoolError::UnknownBackend(_)
        )
    }

    pub(super) fn log_access(req: &RequestEnvelope, status: u16) {
        let trace_id = req.trace_id.as_deref().unwrap_or("-");
        let span_id = req.span_id.as_deref().unwrap_or("-");
        let latency_ms = req.start.elapsed().as_millis() as u64;
        if req.routing_transparency_enabled {
            let reason = if req.routing_transparency_include_reason {
                req.route_reason.as_deref().unwrap_or("-")
            } else {
                "-"
            };
            info!(
                "request_id={} route_upstream={} route_path_len={} route_host_specific={} route_reason={} lb={}",
                req.request_id,
                req.upstream_name.as_deref().unwrap_or("-"),
                req.route_path_len.unwrap_or_default(),
                req.route_host_specific.unwrap_or(false),
                reason,
                req.backend_lb.as_deref().unwrap_or("-")
            );
        }

        if let Some(span) = req.trace_span.as_ref() {
            span.in_scope(|| match req.error_kind.as_ref() {
                Some(e) => tracing::warn!(
                    request_id = req.request_id,
                    trace_id = trace_id,
                    span_id = span_id,
                    method = %req.method,
                    path = %req.path,
                    status = status,
                    backend = %req.backend_addr.as_deref().unwrap_or("-"),
                    upstream = %req.upstream_name.as_deref().unwrap_or("-"),
                    latency_ms = latency_ms,
                    retries = req.retry_count,
                    error = %e,
                    "request completed with error"
                ),
                None => tracing::info!(
                    request_id = req.request_id,
                    trace_id = trace_id,
                    span_id = span_id,
                    method = %req.method,
                    path = %req.path,
                    status = status,
                    backend = %req.backend_addr.as_deref().unwrap_or("-"),
                    upstream = %req.upstream_name.as_deref().unwrap_or("-"),
                    latency_ms = latency_ms,
                    retries = req.retry_count,
                    "request completed"
                ),
            });
        }

        match req.error_kind {
            Some(e) => info!(
                "request_id={} trace_id={} span_id={} method={} path={} status={} backend={} upstream={} latency_ms={} retries={} error={}",
                req.request_id,
                trace_id,
                span_id,
                req.method,
                req.path,
                status,
                req.backend_addr.as_deref().unwrap_or("-"),
                req.upstream_name.as_deref().unwrap_or("-"),
                latency_ms,
                req.retry_count,
                e,
            ),
            None => info!(
                "request_id={} trace_id={} span_id={} method={} path={} status={} backend={} upstream={} latency_ms={} retries={}",
                req.request_id,
                trace_id,
                span_id,
                req.method,
                req.path,
                status,
                req.backend_addr.as_deref().unwrap_or("-"),
                req.upstream_name.as_deref().unwrap_or("-"),
                latency_ms,
                req.retry_count,
            ),
        }
    }

    pub(super) fn request_outcome_route_target(req: &RequestEnvelope) -> RouteOutcomeTarget<'_> {
        RouteOutcomeTarget {
            route: req.upstream_name.as_deref().unwrap_or("unrouted"),
        }
    }

    pub(super) fn request_outcome_backend_target(
        req: &RequestEnvelope,
    ) -> Option<BackendOutcomeTarget<'_>> {
        req.upstream_name
            .as_deref()
            .map(|upstream| BackendOutcomeTarget {
                upstream,
                backend_addr: req.backend_addr.as_deref(),
                backend_index: req.backend_index,
            })
    }
}
