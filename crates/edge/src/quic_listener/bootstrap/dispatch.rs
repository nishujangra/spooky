use std::convert::Infallible;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;
use impulse_errors::ProxyError;

use super::{
    context::BootstrapDispatchCtx,
    intake::bootstrap_error_response,
    outcome::observe_bootstrap_dispatch_failure,
    request::{
        BootstrapBackendFailureReason, BootstrapLifecycleStage, BootstrapPreparedRoute,
        BootstrapTerminalOutcome, BootstrapTerminalResponse, BootstrapTerminalResult,
        BootstrapTimeoutReason,
    },
    websocket::dispatch_bootstrap_websocket,
};

pub(in crate::quic_listener) struct BootstrapDispatchInput<'a> {
    pub(in crate::quic_listener) upstream_req: Request<BoxBody<Bytes, Infallible>>,
    pub(in crate::quic_listener) prepared_route: &'a BootstrapPreparedRoute,
    pub(in crate::quic_listener) dispatch_ctx: BootstrapDispatchCtx<'a>,
}

async fn dispatch_bootstrap_http(
    input: BootstrapDispatchInput<'_>,
) -> BootstrapTerminalResult<Response<Incoming>> {
    match input
        .dispatch_ctx
        .request
        .runtime
        .transport_pool
        .send_backend_request(&input.prepared_route.backend_addr, input.upstream_req)
        .await
    {
        Ok(response) => Ok(response),
        Err(proxy_err) => {
            let status = match proxy_err {
                ProxyError::Timeout => StatusCode::GATEWAY_TIMEOUT,
                _ => StatusCode::BAD_GATEWAY,
            };
            observe_bootstrap_dispatch_failure(
                input.prepared_route,
                input.dispatch_ctx.request.runtime.metrics.as_ref(),
                input.dispatch_ctx.request.request_start,
                input.dispatch_ctx.request_id,
                status,
                &proxy_err,
            );
            Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                if matches!(proxy_err, ProxyError::Timeout) {
                    BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream)
                } else {
                    BootstrapTerminalOutcome::BackendFailed(
                        BootstrapBackendFailureReason::DispatchFailed,
                    )
                },
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    status,
                    if matches!(proxy_err, ProxyError::Timeout) {
                        b"upstream timeout\n"
                    } else {
                        b"upstream error\n"
                    },
                ),
            ))
        }
    }
}

pub(in crate::quic_listener) async fn dispatch_bootstrap_upstream(
    input: BootstrapDispatchInput<'_>,
) -> BootstrapTerminalResult<Response<Incoming>> {
    if input.dispatch_ctx.is_websocket_upgrade {
        dispatch_bootstrap_websocket(input).await
    } else {
        dispatch_bootstrap_http(input).await
    }
}
