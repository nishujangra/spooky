use std::convert::Infallible;

use bytes::Bytes;
use http::{Request, Response, StatusCode, Uri, response::Builder as ResponseBuilder};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::{body::Incoming, client::conn::http1 as client_http1, upgrade, upgrade::OnUpgrade};
use hyper_util::rt::TokioIo;
use log::{debug, warn};
use impulse_config::backend_endpoint::BackendScheme;
use impulse_errors::ProxyError;

use super::{
    dispatch::BootstrapDispatchInput,
    intake::bootstrap_error_response,
    outcome::{finish_bootstrap_backend_request_accounting, observe_bootstrap_dispatch_failure},
    request::{
        BootstrapBackendFailureReason, BootstrapLifecycleStage, BootstrapPreparedRoute,
        BootstrapTerminalOutcome, BootstrapTerminalResponse, BootstrapTerminalResult,
        BootstrapTimeoutReason,
    },
};
use crate::quic_listener::protocol::is_websocket_upgrade_request;

pub(in crate::quic_listener) struct BootstrapWebsocketFlow {
    pub(in crate::quic_listener) is_websocket_upgrade: bool,
    pub(in crate::quic_listener) client_upgrade: Option<OnUpgrade>,
}

fn boxed_full(body: Bytes) -> BoxBody<Bytes, Infallible> {
    Full::new(body).map_err(|never| match never {}).boxed()
}

pub(in crate::quic_listener) fn capture_bootstrap_websocket_flow(
    req: &mut Request<Incoming>,
    use_h2: bool,
) -> BootstrapWebsocketFlow {
    let is_websocket_upgrade = is_websocket_upgrade_request(req, use_h2);
    let client_upgrade = if is_websocket_upgrade {
        Some(upgrade::on(&mut *req))
    } else {
        None
    };

    BootstrapWebsocketFlow {
        is_websocket_upgrade,
        client_upgrade,
    }
}

pub(in crate::quic_listener) async fn dispatch_bootstrap_websocket(
    input: BootstrapDispatchInput<'_>,
) -> BootstrapTerminalResult<Response<Incoming>> {
    if input.prepared_route.endpoint.scheme() != BackendScheme::Http {
        return Err(BootstrapTerminalResponse::new(
            BootstrapLifecycleStage::Dispatch,
            BootstrapTerminalOutcome::BackendFailed(BootstrapBackendFailureReason::DispatchFailed),
            bootstrap_error_response(
                &input.dispatch_ctx.request.runtime.alt_svc,
                StatusCode::BAD_GATEWAY,
                b"websocket bootstrap requires http upstream\n",
            ),
        ));
    }

    let backend_target = input.prepared_route.endpoint.authority().to_string();
    let upstream_path_uri = match Uri::try_from(input.dispatch_ctx.request_path) {
        Ok(uri) => uri,
        Err(_) => {
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                BootstrapTerminalOutcome::BackendFailed(
                    BootstrapBackendFailureReason::RequestBuildFailed,
                ),
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    StatusCode::BAD_GATEWAY,
                    b"bad uri\n",
                ),
            ));
        }
    };
    let (mut parts, body) = input.upstream_req.into_parts();
    parts.uri = upstream_path_uri;
    let upstream_req = Request::from_parts(parts, body);

    let stream = match tokio::time::timeout(
        input.dispatch_ctx.request.runtime.backend_timeout,
        tokio::net::TcpStream::connect(&backend_target),
    )
    .await
    {
        Ok(Ok(stream)) => {
            if let Ok(resolved_addr) = stream.peer_addr() {
                input
                    .dispatch_ctx
                    .request
                    .runtime
                    .metrics
                    .record_backend_connect(
                        &backend_target,
                        input.prepared_route.endpoint.authority_host(),
                        resolved_addr,
                    );
            }
            stream
        }
        Ok(Err(err)) => {
            warn!("Bootstrap WebSocket connect error: {}", err);
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                BootstrapTerminalOutcome::BackendFailed(
                    BootstrapBackendFailureReason::DispatchFailed,
                ),
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    StatusCode::BAD_GATEWAY,
                    b"upstream error\n",
                ),
            ));
        }
        Err(_) => {
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream),
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    StatusCode::GATEWAY_TIMEOUT,
                    b"upstream timeout\n",
                ),
            ));
        }
    };

    let io = TokioIo::new(stream);
    let (mut sender, conn) = match client_http1::handshake(io).await {
        Ok(v) => v,
        Err(err) => {
            warn!("Bootstrap WebSocket handshake setup failed: {}", err);
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                BootstrapTerminalOutcome::BackendFailed(
                    BootstrapBackendFailureReason::DispatchFailed,
                ),
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    StatusCode::BAD_GATEWAY,
                    b"upstream error\n",
                ),
            ));
        }
    };
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    match tokio::time::timeout(
        input.dispatch_ctx.request.runtime.backend_timeout,
        sender.send_request(upstream_req),
    )
    .await
    {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(err)) => {
            let proxy_err = ProxyError::Transport(err.to_string());
            observe_bootstrap_dispatch_failure(
                input.prepared_route,
                input.dispatch_ctx.request.runtime.metrics.as_ref(),
                input.dispatch_ctx.request.request_start,
                input.dispatch_ctx.request_id,
                StatusCode::BAD_GATEWAY,
                &proxy_err,
            );
            Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                BootstrapTerminalOutcome::BackendFailed(
                    BootstrapBackendFailureReason::DispatchFailed,
                ),
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    StatusCode::BAD_GATEWAY,
                    b"upstream error\n",
                ),
            ))
        }
        Err(_) => {
            observe_bootstrap_dispatch_failure(
                input.prepared_route,
                input.dispatch_ctx.request.runtime.metrics.as_ref(),
                input.dispatch_ctx.request.request_start,
                input.dispatch_ctx.request_id,
                StatusCode::GATEWAY_TIMEOUT,
                &ProxyError::Timeout,
            );
            Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::Dispatch,
                BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream),
                bootstrap_error_response(
                    &input.dispatch_ctx.request.runtime.alt_svc,
                    StatusCode::GATEWAY_TIMEOUT,
                    b"upstream timeout\n",
                ),
            ))
        }
    }
}

pub(in crate::quic_listener) fn write_bootstrap_websocket_upgrade(
    resp_builder: ResponseBuilder,
    upstream_resp: &mut Response<Incoming>,
    prepared_route: &BootstrapPreparedRoute,
    request_start: std::time::Instant,
    alt_svc: &str,
    client_upgrade: Option<OnUpgrade>,
    status: StatusCode,
) -> Result<Response<BoxBody<Bytes, Infallible>>, hyper::Error> {
    let client_upgrade = match client_upgrade {
        Some(upgrade) => upgrade,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("alt-svc", alt_svc)
                .body(boxed_full(Bytes::from_static(b"upgrade setup error\n")))
                .unwrap_or_else(|_| Response::new(boxed_full(Bytes::from_static(b"error\n")))));
        }
    };
    let upstream_upgrade = upgrade::on(upstream_resp);
    tokio::spawn(async move {
        let (client, upstream) = match tokio::try_join!(client_upgrade, upstream_upgrade) {
            Ok(v) => v,
            Err(err) => {
                debug!("Bootstrap WebSocket upgrade join failed: {}", err);
                return;
            }
        };
        let mut client = TokioIo::new(client);
        let mut upstream = TokioIo::new(upstream);
        let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    });
    finish_bootstrap_backend_request_accounting(
        prepared_route,
        request_start,
        Some(status.as_u16()),
    );
    Ok(resp_builder
        .body(boxed_full(Bytes::new()))
        .unwrap_or_else(|_| Response::new(boxed_full(Bytes::new()))))
}
