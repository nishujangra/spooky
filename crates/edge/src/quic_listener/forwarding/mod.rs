mod auth;
mod dispatch;
mod lb_key;
mod prepare;
mod resolve;
mod response;
mod stream_progress;

use std::convert::Infallible;

use http_body_util::Full;
use spooky_config::config::ScopedRateLimitScope;
use spooky_errors::ClassifiedUpstreamProxyError;

use self::prepare::{RequestFinalizationConfig, StartedRequestEnvelope};
#[cfg(test)]
pub(in crate::quic_listener) use self::resolve::TargetResolutionRequest as TestTargetResolutionRequest;
pub(in crate::quic_listener) use self::{
    auth::evaluate_pending_forward_external_auth, resolve::BootstrapTargetResolutionInput,
};
use super::*;
use crate::runtime::connection::{
    outcome::{
        BackendOutcomeTarget, RouteOutcomeTarget, observe_admission_outcome,
        observe_proxy_error_outcome,
    },
    request::PendingForward,
    stream::{
        AdmissionPermits, BackendFailureReason, RejectionReason, StreamPhase, TerminalReason,
    },
};

pub(super) fn terminalize_stream(
    req: &mut RequestEnvelope,
    reason: TerminalReason,
    metrics: &Metrics,
) -> StreamPhase {
    req.transition_to_terminal_with_cleanup(reason, metrics)
}

pub(super) fn backend_failure_reason_for_proxy_error(err: &ProxyError) -> BackendFailureReason {
    match err {
        ProxyError::Timeout => BackendFailureReason::UpstreamTimeout,
        ProxyError::Tls(_) => BackendFailureReason::UpstreamTls,
        ProxyError::Transport(_) | ProxyError::Pool(_) => BackendFailureReason::UpstreamTransport,
        ProxyError::Protocol(_) => BackendFailureReason::UpstreamProtocol,
        ProxyError::Bridge(_) => BackendFailureReason::UpstreamBridge,
    }
}

pub(super) fn rejection_reason_for_status(status: http::StatusCode) -> RejectionReason {
    match status {
        http::StatusCode::PAYLOAD_TOO_LARGE => RejectionReason::RequestBodyTooLarge,
        http::StatusCode::TOO_MANY_REQUESTS => RejectionReason::RateLimited,
        http::StatusCode::SERVICE_UNAVAILABLE => RejectionReason::Overloaded,
        http::StatusCode::BAD_REQUEST => RejectionReason::ValidationFailed,
        _ => RejectionReason::ValidationFailed,
    }
}

// Shared forwarding dependencies passed through extracted submodules.
pub(in crate::quic_listener) struct ForwardingSharedCtx<'a> {
    pub(in crate::quic_listener) metrics: Arc<Metrics>,
    pub(in crate::quic_listener) resilience: &'a RuntimeResilience,
    pub(in crate::quic_listener) routing_index: &'a RouteIndex,
    pub(in crate::quic_listener) upstream_pools: &'a HashMap<String, Arc<RwLock<UpstreamPool>>>,
}

pub(in crate::quic_listener) struct ForwardingExecutionCtx<'a> {
    pub(in crate::quic_listener) transport_pool: Arc<UpstreamTransportPool>,
    pub(in crate::quic_listener) backend_endpoints: Arc<HashMap<String, BackendEndpoint>>,
    pub(in crate::quic_listener) upstream_inflight: &'a HashMap<String, Arc<Semaphore>>,
    pub(in crate::quic_listener) global_inflight: Arc<Semaphore>,
    pub(in crate::quic_listener) backend_timeout: Duration,
    pub(in crate::quic_listener) inflight_acquire_wait: Duration,
}

pub(in crate::quic_listener) struct StreamProgressConfig {
    pub(in crate::quic_listener) backend_body_idle_timeout: Duration,
    pub(in crate::quic_listener) backend_body_total_timeout: Duration,
    pub(in crate::quic_listener) max_response_body_bytes: usize,
    pub(in crate::quic_listener) unknown_length_response_prebuffer_bytes: usize,
    pub(in crate::quic_listener) client_body_idle_timeout: Duration,
    pub(in crate::quic_listener) listen_port: u16,
}

impl QUICListener {
    pub(super) fn log_classified_upstream_failure(
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

    fn is_internal_pool_control_error(error: &PoolError) -> bool {
        matches!(
            error,
            PoolError::InflightLimiterClosed | PoolError::UnknownBackend(_)
        )
    }

    fn log_access(req: &RequestEnvelope, status: u16) {
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

    fn request_outcome_route_target(req: &RequestEnvelope) -> RouteOutcomeTarget<'_> {
        RouteOutcomeTarget {
            route: req.upstream_name.as_deref().unwrap_or("unrouted"),
        }
    }

    fn request_outcome_backend_target(req: &RequestEnvelope) -> Option<BackendOutcomeTarget<'_>> {
        req.upstream_name
            .as_deref()
            .map(|upstream| BackendOutcomeTarget {
                upstream,
                backend_addr: req.backend_addr.as_deref(),
                backend_index: req.backend_index,
            })
    }

    fn materialize_forward_after_auth(
        stream_id: u64,
        req: &mut RequestEnvelope,
        h3: &mut quiche::h3::Connection,
        quic: &mut quiche::Connection,
        exec_ctx: &ForwardingExecutionCtx<'_>,
        shared_ctx: &ForwardingSharedCtx<'_>,
    ) -> Result<bool, quiche::h3::Error> {
        let metrics = shared_ctx.metrics.as_ref();
        let resilience = shared_ctx.resilience;
        let Some(pending_forward) = req.pending_forward().cloned() else {
            let _ = observe_proxy_error_outcome(
                metrics,
                RouteOutcomeTarget::UNROUTED,
                None,
                req.start.elapsed(),
                Some(http::StatusCode::INTERNAL_SERVER_ERROR),
                &ProxyError::Transport("missing deferred forward snapshot".into()),
                None,
            );
            Self::send_simple_response(
                h3,
                quic,
                stream_id,
                http::StatusCode::INTERNAL_SERVER_ERROR,
                b"missing deferred forward snapshot\n",
            )?;
            terminalize_stream(
                req,
                TerminalReason::Rejected(RejectionReason::ValidationFailed),
                metrics,
            );
            return Ok(false);
        };
        let Some(upstream_name) = req.upstream_name.clone() else {
            let _ = observe_proxy_error_outcome(
                metrics,
                RouteOutcomeTarget::UNROUTED,
                None,
                req.start.elapsed(),
                Some(http::StatusCode::INTERNAL_SERVER_ERROR),
                &ProxyError::Transport("missing upstream route".into()),
                None,
            );
            Self::send_simple_response(
                h3,
                quic,
                stream_id,
                http::StatusCode::INTERNAL_SERVER_ERROR,
                b"missing upstream route\n",
            )?;
            terminalize_stream(
                req,
                TerminalReason::Rejected(RejectionReason::ValidationFailed),
                metrics,
            );
            return Ok(false);
        };

        let (
            backend_index,
            upstream_pool,
            global_permit,
            upstream_permit,
            adaptive_permit,
            route_queue_permit,
        ) = match crate::quic_listener::admission::execute_forwarding_post_auth_admission(
            resilience,
            &upstream_name,
            req.upstream_pool.as_ref(),
            req.backend_index,
            pending_forward.backend_index,
            exec_ctx.upstream_inflight,
            Arc::clone(&exec_ctx.global_inflight),
            exec_ctx.inflight_acquire_wait,
        ) {
            crate::quic_listener::admission::PostAuthAdmissionExecution::Ready(ready) => {
                if ready.waited_for_global_permit {
                    metrics.inc_inflight_wait_admit_global();
                }
                if ready.waited_for_upstream_permit {
                    metrics.inc_inflight_wait_admit_upstream();
                }
                (
                    ready.backend_index,
                    ready.upstream_pool,
                    ready.global_permit,
                    ready.upstream_permit,
                    ready.adaptive_permit,
                    ready.route_queue_permit,
                )
            }
            crate::quic_listener::admission::PostAuthAdmissionExecution::Rejected(
                crate::quic_listener::admission::PostAuthAdmissionRejection::Overloaded(decision),
            ) => {
                let _ = observe_admission_outcome(
                    metrics,
                    RouteOutcomeTarget {
                        route: &upstream_name,
                    },
                    Some(BackendOutcomeTarget {
                        upstream: &upstream_name,
                        backend_addr: Some(pending_forward.backend_addr.as_ref()),
                        backend_index: Some(pending_forward.backend_index),
                    }),
                    req.start.elapsed(),
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    crate::runtime::connection::outcome::AdmissionOutcomeClass::OverloadShed {
                        reason: Some(decision.reason.metrics_reason()),
                    },
                );
                Self::send_overload_response(
                    h3,
                    quic,
                    stream_id,
                    decision.body,
                    decision.retry_after_seconds,
                )?;
                resilience
                    .adaptive_admission
                    .observe(req.start.elapsed(), true);
                req.set_terminal_overload_reason(Some(decision.reason.metrics_reason()));
                req.mark_terminal_outcome_recorded();
                terminalize_stream(
                    req,
                    TerminalReason::Rejected(RejectionReason::Overloaded),
                    metrics,
                );
                return Ok(false);
            }
            crate::quic_listener::admission::PostAuthAdmissionExecution::Rejected(
                crate::quic_listener::admission::PostAuthAdmissionRejection::Failed(decision),
            ) => {
                let outcome = if let Some(reason) = decision.overload_reason {
                    crate::runtime::connection::outcome::AdmissionOutcomeClass::OverloadShed {
                        reason: Some(reason.metrics_reason()),
                    }
                } else {
                    crate::runtime::connection::outcome::AdmissionOutcomeClass::Failed {
                        timed_out: matches!(decision.route_outcome, Some(RouteOutcome::Timeout)),
                    }
                };
                let _ = observe_admission_outcome(
                    metrics,
                    RouteOutcomeTarget {
                        route: &upstream_name,
                    },
                    Some(BackendOutcomeTarget {
                        upstream: &upstream_name,
                        backend_addr: Some(pending_forward.backend_addr.as_ref()),
                        backend_index: Some(pending_forward.backend_index),
                    }),
                    req.start.elapsed(),
                    decision.status,
                    outcome,
                );
                Self::send_simple_response(h3, quic, stream_id, decision.status, decision.body)?;
                if decision.observe_adaptive_overload {
                    resilience
                        .adaptive_admission
                        .observe(req.start.elapsed(), true);
                }
                if let Some(reason) = decision.overload_reason {
                    req.set_terminal_overload_reason(Some(reason.metrics_reason()));
                }
                req.mark_terminal_outcome_recorded();
                terminalize_stream(
                    req,
                    TerminalReason::Rejected(rejection_reason_for_status(decision.status)),
                    metrics,
                );
                return Ok(false);
            }
        };
        req.transition_to_admitted(AdmissionPermits {
            global: global_permit,
            upstream: upstream_permit,
            adaptive: adaptive_permit,
            route_queue: route_queue_permit,
        });

        let Some(backend_endpoint) = exec_ctx
            .backend_endpoints
            .get(pending_forward.backend_addr.as_ref())
            .cloned()
        else {
            let _ = observe_proxy_error_outcome(
                metrics,
                RouteOutcomeTarget {
                    route: &upstream_name,
                },
                Some(BackendOutcomeTarget {
                    upstream: &upstream_name,
                    backend_addr: Some(pending_forward.backend_addr.as_ref()),
                    backend_index: Some(backend_index),
                }),
                req.start.elapsed(),
                Some(http::StatusCode::BAD_GATEWAY),
                &ProxyError::Transport("unknown backend endpoint".into()),
                None,
            );
            Self::send_simple_response(
                h3,
                quic,
                stream_id,
                http::StatusCode::BAD_GATEWAY,
                b"unknown backend endpoint\n",
            )?;
            terminalize_stream(
                req,
                TerminalReason::BackendFailed(BackendFailureReason::DispatchSpawnFailed),
                metrics,
            );
            return Ok(false);
        };

        let request_mode = req.request_mode();
        let websocket_h1_tunnel = req.tunnel_mode == TunnelMode::Websocket
            && backend_endpoint.scheme() == BackendScheme::Http;
        let (body_tx, websocket_tunnel_body_rx, request_body) = if request_mode.bodyless_mode() {
            (None, None, Some(BoxBody::new(Full::new(Bytes::new()))))
        } else if websocket_h1_tunnel {
            let (tx, rx) = mpsc::channel::<Bytes>(REQUEST_CHUNK_CHANNEL_CAPACITY);
            (Some(tx), Some(rx), None)
        } else {
            let (tx, channel_body) = ChannelBody::channel(REQUEST_CHUNK_CHANNEL_CAPACITY);
            (Some(tx), None, Some(channel_body.boxed()))
        };

        let request = if websocket_h1_tunnel {
            None
        } else {
            match pending_forward.build_request(
                &backend_endpoint,
                // Non-websocket branches above always set a body; fall back to an
                // empty body rather than panicking if that invariant ever changes.
                request_body.unwrap_or_else(|| BoxBody::new(Full::new(Bytes::new()))),
                None,
            ) {
                Ok(request) => Some(request),
                Err(err) => {
                    let err_text = err.to_string();
                    let _ = observe_proxy_error_outcome(
                        metrics,
                        RouteOutcomeTarget {
                            route: &upstream_name,
                        },
                        Some(BackendOutcomeTarget {
                            upstream: &upstream_name,
                            backend_addr: Some(pending_forward.backend_addr.as_ref()),
                            backend_index: Some(backend_index),
                        }),
                        req.start.elapsed(),
                        Some(http::StatusCode::BAD_REQUEST),
                        &err,
                        None,
                    );
                    Self::send_simple_response(
                        h3,
                        quic,
                        stream_id,
                        http::StatusCode::BAD_REQUEST,
                        b"invalid request\n",
                    )?;
                    error!("failed to build upstream request after auth: {}", err_text);
                    resilience
                        .adaptive_admission
                        .observe(req.start.elapsed(), true);
                    terminalize_stream(
                        req,
                        TerminalReason::Rejected(RejectionReason::ValidationFailed),
                        metrics,
                    );
                    return Ok(false);
                }
            }
        };

        let result_rx = match Self::spawn_upstream_forward_task(
            req,
            Arc::clone(&pending_forward),
            backend_endpoint,
            request,
            websocket_tunnel_body_rx,
            exec_ctx,
            shared_ctx,
        ) {
            Ok(result_rx) => result_rx,
            Err(err) => {
                let _ = observe_proxy_error_outcome(
                    metrics,
                    RouteOutcomeTarget {
                        route: &upstream_name,
                    },
                    Some(BackendOutcomeTarget {
                        upstream: &upstream_name,
                        backend_addr: Some(pending_forward.backend_addr.as_ref()),
                        backend_index: Some(backend_index),
                    }),
                    req.start.elapsed(),
                    Some(http::StatusCode::SERVICE_UNAVAILABLE),
                    &ProxyError::Transport("upstream runtime unavailable".into()),
                    None,
                );
                Self::send_simple_response(
                    h3,
                    quic,
                    stream_id,
                    http::StatusCode::SERVICE_UNAVAILABLE,
                    b"upstream runtime unavailable
",
                )?;
                error!("failed to spawn upstream task after auth: {}", err);
                resilience
                    .adaptive_admission
                    .observe(req.start.elapsed(), true);
                terminalize_stream(
                    req,
                    TerminalReason::BackendFailed(BackendFailureReason::DispatchSpawnFailed),
                    metrics,
                );
                return Ok(false);
            }
        };
        if let Ok(pool) = upstream_pool.write() {
            pool.begin_request_for_accounting(backend_index);
        }
        req.transition_admitted_to_awaiting_upstream(body_tx, result_rx);
        let _ = Self::flush_request_buffer(req, metrics);
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn flush_send(
        socket: &UdpSocket,
        send_buf: &mut [u8],
        connection: &mut QuicConnection,
    ) {
        let mut packet_count = 0;

        loop {
            match connection.quic.send(send_buf) {
                Ok((write, send_info)) => {
                    packet_count += 1;
                    debug!("Sending {} bytes to {}", write, send_info.to);
                    if let Err(e) = socket.send_to(&send_buf[..write], send_info.to) {
                        error!("Failed to send UDP packet: {:?}", e);
                        break;
                    }
                }
                Err(quiche::Error::Done) => break,
                Err(e) => {
                    error!("QUIC send failed: {:?}", e);
                    break;
                }
            }
        }

        if packet_count > 0 {
            debug!("Sent {} packets", packet_count);
        }
    }
}

impl QUICListener {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn handle_h3(
        connection: &mut QuicConnection,
        transport_pool: Arc<UpstreamTransportPool>,
        backend_endpoints: Arc<HashMap<String, BackendEndpoint>>,
        upstream_policies: Arc<HashMap<String, RuntimeUpstreamPolicy>>,
        upstream_pools: &HashMap<String, Arc<RwLock<UpstreamPool>>>,
        upstream_inflight: &HashMap<String, Arc<Semaphore>>,
        global_inflight: Arc<Semaphore>,
        backend_timeout: Duration,
        backend_body_idle_timeout: Duration,
        backend_body_total_timeout: Duration,
        backend_total_request_timeout: Duration,
        routing_index: &RouteIndex,
        metrics: Arc<Metrics>,
        resilience: &RuntimeResilience,
        max_request_body_bytes: usize,
        max_response_body_bytes: usize,
        request_buffer_global_cap_bytes: usize,
        unknown_length_response_prebuffer_bytes: usize,
        client_body_idle_timeout: Duration,
        inflight_acquire_wait: Duration,
        tracing_enabled: bool,
        routing_transparency_enabled: bool,
        routing_transparency_include_reason: bool,
        listen_port: u16,
        max_streams_per_connection: usize,
    ) -> Result<(), quiche::h3::Error> {
        let mut body_buf = [0u8; MAX_DATAGRAM_SIZE_BYTES];

        if connection.h3.is_none() {
            connection.h3 = Some(quiche::h3::Connection::with_transport(
                &mut connection.quic,
                &connection.h3_config,
            )?);
        }

        let h3 = match connection.h3.as_mut() {
            Some(h3) => h3,
            None => return Ok(()),
        };
        let shared_ctx = ForwardingSharedCtx {
            metrics: Arc::clone(&metrics),
            resilience,
            routing_index,
            upstream_pools,
        };
        let exec_ctx = ForwardingExecutionCtx {
            transport_pool: Arc::clone(&transport_pool),
            backend_endpoints: Arc::clone(&backend_endpoints),
            upstream_inflight,
            global_inflight: Arc::clone(&global_inflight),
            backend_timeout,
            inflight_acquire_wait,
        };
        let progress_config = StreamProgressConfig {
            backend_body_idle_timeout,
            backend_body_total_timeout,
            max_response_body_bytes,
            unknown_length_response_prebuffer_bytes,
            client_body_idle_timeout,
            listen_port,
        };

        loop {
            match h3.poll(&mut connection.quic) {
                Ok((stream_id, quiche::h3::Event::Headers { list, .. })) => {
                    let request = match validate_request_headers(&list, resilience) {
                        Ok(request) => request,
                        Err((status, body, is_policy)) => {
                            metrics.inc_request_validation_reject();
                            if is_policy {
                                metrics.inc_policy_denied();
                            }
                            let _ = observe_proxy_error_outcome(
                                &metrics,
                                RouteOutcomeTarget::UNROUTED,
                                None,
                                Duration::from_millis(0),
                                Some(status),
                                &ProxyError::Bridge(spooky_errors::BridgeError::InvalidHeader),
                                None,
                            );
                            let _ = Self::send_simple_response(
                                h3,
                                &mut connection.quic,
                                stream_id,
                                status,
                                body,
                            );
                            continue;
                        }
                    };
                    let method = request.method;
                    let path = request.path;
                    let authority = request.authority;
                    let content_length = request.content_length;
                    let websocket_tunnel = request.websocket_tunnel;
                    let tunnel_mode = if websocket_tunnel {
                        TunnelMode::Websocket
                    } else if is_connect_method(&method) {
                        TunnelMode::Connect
                    } else {
                        TunnelMode::None
                    };

                    metrics.inc_total();
                    let request_start = Instant::now();

                    if connection.quic.is_in_early_data() {
                        if resilience.early_data_allowed_for(&method) {
                            metrics.inc_early_data_accepted();
                        } else {
                            metrics.inc_early_data_rejected();
                            metrics.inc_policy_denied();
                            let _ = observe_proxy_error_outcome(
                                &metrics,
                                RouteOutcomeTarget::UNROUTED,
                                None,
                                request_start.elapsed(),
                                Some(http::StatusCode::TOO_EARLY),
                                &ProxyError::Transport(
                                    "request blocked by early-data policy".into(),
                                ),
                                None,
                            );
                            Self::send_simple_response(
                                h3,
                                &mut connection.quic,
                                stream_id,
                                http::StatusCode::TOO_EARLY,
                                b"request blocked by early-data policy\n",
                            )?;
                            continue;
                        }
                    }

                    // App-level stream count cap: mirrors the QUIC max_streams_bidi
                    // limit so the streams HashMap can never grow beyond what the
                    // transport layer allows even if a race or misconfiguration
                    // delivers a stream-open event before the flow-control frame
                    // reaches the client.
                    if connection.streams.len() >= max_streams_per_connection {
                        warn!(
                            "stream limit reached ({} streams), rejecting stream {}",
                            max_streams_per_connection, stream_id
                        );
                        Self::send_simple_response(
                            h3,
                            &mut connection.quic,
                            stream_id,
                            http::StatusCode::SERVICE_UNAVAILABLE,
                            b"too many concurrent streams\n",
                        )?;
                        continue;
                    }

                    // Route lookup now only selects an upstream/backend; actual
                    // backend/inflight admission is deferred until local auth
                    // succeeds immediately or async external auth completes.
                    let sticky_cid_key = hex::encode(connection.primary_scid.as_ref());
                    let quic_trace_id = connection.quic.trace_id().to_string();
                    let pre_auth = match Self::prepare_request_for_auth(
                        stream_id,
                        h3,
                        &mut connection.quic,
                        connection.peer_address,
                        &quic_trace_id,
                        request_start,
                        &method,
                        &path,
                        authority.as_deref(),
                        content_length,
                        tunnel_mode,
                        &list,
                        sticky_cid_key.as_str(),
                        tracing_enabled,
                        routing_index,
                        &upstream_policies,
                        upstream_pools,
                        &metrics,
                        resilience,
                    )? {
                        Some(pre_auth) => pre_auth,
                        None => continue,
                    };
                    let started_auth = match Self::start_request_auth(
                        stream_id,
                        h3,
                        &mut connection.quic,
                        request_start,
                        &metrics,
                        pre_auth,
                        RequestFinalizationConfig {
                            routing_transparency_enabled,
                            routing_transparency_include_reason,
                            backend_total_request_timeout,
                        },
                    )? {
                        Some(started_auth) => started_auth,
                        None => continue,
                    };
                    let StartedRequestEnvelope {
                        envelope,
                        should_materialize_forward,
                    } = started_auth;
                    connection.streams.insert(stream_id, envelope);
                    if should_materialize_forward {
                        let keep_stream = if let Some(req) = connection.streams.get_mut(&stream_id)
                        {
                            Self::materialize_forward_after_auth(
                                stream_id,
                                req,
                                h3,
                                &mut connection.quic,
                                &exec_ctx,
                                &shared_ctx,
                            )?
                        } else {
                            false
                        };
                        if !keep_stream {
                            if let Some(req) = connection.streams.get_mut(&stream_id)
                                && !req.execution.is_terminal()
                            {
                                terminalize_stream(
                                    req,
                                    TerminalReason::Cancelled(CancellationReason::OperatorAbort),
                                    &metrics,
                                );
                            }
                            connection.streams.remove(&stream_id);
                            continue;
                        }
                    }
                    if let Some(req) = connection.streams.get(&stream_id) {
                        debug!(
                            "request_id={} method={} path={} stream_id={}",
                            req.request_id, req.method, req.path, stream_id
                        );
                    }
                }
                Ok((stream_id, quiche::h3::Event::Data)) => loop {
                    match h3.recv_body(&mut connection.quic, stream_id, &mut body_buf) {
                        Ok(read) => {
                            let mut shed_due_to_buffer_pressure = false;
                            let mut reject_body_for_bodyless = None::<(String, Duration)>;
                            let mut payload_too_large = None::<(String, Duration)>;
                            if let Some(req) = connection.streams.get_mut(&stream_id) {
                                if read > 0 {
                                    req.set_last_body_activity(Instant::now());
                                }
                                if req.request_mode().bodyless_mode() && read > 0 {
                                    reject_body_for_bodyless = Some((
                                        req.upstream_name
                                            .clone()
                                            .unwrap_or_else(|| "unrouted".to_string()),
                                        req.start.elapsed(),
                                    ));
                                }
                                if reject_body_for_bodyless.is_none() {
                                    let next_state = checked_request_body_ingress(
                                        RequestBodyGuardrailConfig {
                                            idle_timeout: Duration::ZERO,
                                            total_timeout: Duration::ZERO,
                                            max_body_bytes: max_request_body_bytes,
                                            max_buffered_bytes: usize::MAX,
                                        },
                                        RequestBodyGuardrailInput {
                                            elapsed: req.start.elapsed(),
                                            idle_for: Instant::now().saturating_duration_since(
                                                req.last_body_activity(),
                                            ),
                                            bytes_received: req.body_bytes_received(),
                                            buffered_bytes: 0,
                                            next_chunk_bytes: read,
                                            declared_content_length: None,
                                            exempt_from_body_size_cap: is_connect_method(
                                                &req.method,
                                            ),
                                        },
                                    );
                                    match next_state {
                                        Err(RequestBodyGuardrailDecision::Reject {
                                            kind: BodyLimitKind::BodySize,
                                        }) => {
                                            payload_too_large = Some((
                                                req.upstream_name
                                                    .clone()
                                                    .unwrap_or_else(|| "unrouted".to_string()),
                                                req.start.elapsed(),
                                            ));
                                        }
                                        Ok(next_state) => {
                                            req.set_body_bytes_received(next_state.bytes_received);

                                            for chunk_slice in
                                                body_buf[..read].chunks(REQUEST_CHUNK_BYTES_LIMIT)
                                            {
                                                let chunk = Bytes::copy_from_slice(chunk_slice);
                                                if let Err(err) = Self::enqueue_request_chunk(
                                                    req,
                                                    chunk,
                                                    &metrics,
                                                    max_request_body_bytes,
                                                    request_buffer_global_cap_bytes,
                                                ) {
                                                    if err == RequestBufferError::BodySize {
                                                        payload_too_large = Some((
                                                            req.upstream_name
                                                                .clone()
                                                                .unwrap_or_else(|| {
                                                                    "unrouted".to_string()
                                                                }),
                                                            req.start.elapsed(),
                                                        ));
                                                    } else {
                                                        shed_due_to_buffer_pressure = true;
                                                        metrics.inc_request_buffer_limit_reject();
                                                        if err == RequestBufferError::Global {
                                                            debug!(
                                                                "global request buffer cap reached"
                                                            );
                                                        }
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                        Err(RequestBodyGuardrailDecision::Reject {
                                            kind:
                                                BodyLimitKind::UnknownLengthPrebuffer
                                                | BodyLimitKind::BufferedBody,
                                        }) => {
                                            shed_due_to_buffer_pressure = true;
                                            metrics.inc_request_buffer_limit_reject();
                                        }
                                        Err(other) => {
                                            unreachable!(
                                                "request ingress should not timeout in data path: {:?}",
                                                other
                                            );
                                        }
                                    }
                                }
                            }
                            if let Some((route_label, elapsed)) = reject_body_for_bodyless {
                                let _ = observe_proxy_error_outcome(
                                    &metrics,
                                    RouteOutcomeTarget {
                                        route: &route_label,
                                    },
                                    None,
                                    elapsed,
                                    Some(http::StatusCode::BAD_REQUEST),
                                    &ProxyError::Bridge(spooky_errors::BridgeError::InvalidHeader),
                                    None,
                                );
                                Self::send_simple_response(
                                    h3,
                                    &mut connection.quic,
                                    stream_id,
                                    http::StatusCode::BAD_REQUEST,
                                    b"request body not allowed for this request\n",
                                )?;
                                if let Some(req) = connection.streams.get_mut(&stream_id) {
                                    terminalize_stream(
                                        req,
                                        TerminalReason::Rejected(
                                            RejectionReason::RequestBodyNotAllowed,
                                        ),
                                        &metrics,
                                    );
                                }
                                connection.streams.remove(&stream_id);
                                resilience.adaptive_admission.observe(elapsed, true);
                                break;
                            }
                            if let Some((route_label, elapsed)) = payload_too_large {
                                let _ = observe_proxy_error_outcome(
                                    &metrics,
                                    RouteOutcomeTarget {
                                        route: &route_label,
                                    },
                                    None,
                                    elapsed,
                                    Some(http::StatusCode::PAYLOAD_TOO_LARGE),
                                    &ProxyError::Transport("request body too large".into()),
                                    None,
                                );
                                Self::send_simple_response(
                                    h3,
                                    &mut connection.quic,
                                    stream_id,
                                    http::StatusCode::PAYLOAD_TOO_LARGE,
                                    REQUEST_BODY_TOO_LARGE_BODY,
                                )?;
                                if let Some(req) = connection.streams.get_mut(&stream_id) {
                                    terminalize_stream(
                                        req,
                                        TerminalReason::Rejected(
                                            RejectionReason::RequestBodyTooLarge,
                                        ),
                                        &metrics,
                                    );
                                }
                                connection.streams.remove(&stream_id);
                                resilience.adaptive_admission.observe(elapsed, true);
                                break;
                            }
                            if shed_due_to_buffer_pressure
                                && let Some(req) = connection.streams.get(&stream_id)
                            {
                                let _ = observe_proxy_error_outcome(
                                    &metrics,
                                    RouteOutcomeTarget {
                                        route: req.upstream_name.as_deref().unwrap_or("unrouted"),
                                    },
                                    Some(BackendOutcomeTarget {
                                        upstream: req
                                            .upstream_name
                                            .as_deref()
                                            .unwrap_or("unrouted"),
                                        backend_addr: req.backend_addr.as_deref(),
                                        backend_index: req.backend_index,
                                    }),
                                    req.start.elapsed(),
                                    Some(http::StatusCode::SERVICE_UNAVAILABLE),
                                    &ProxyError::Pool(PoolError::BackendOverloaded(
                                        "request body backpressure overload".into(),
                                    )),
                                    Some(OverloadShedReason::RequestBufferCap),
                                );
                                Self::send_overload_response(
                                    h3,
                                    &mut connection.quic,
                                    stream_id,
                                    b"request body backpressure overload\n",
                                    resilience.shed_retry_after_seconds,
                                )?;
                                resilience
                                    .adaptive_admission
                                    .observe(req.start.elapsed(), true);
                                if let Some(req) = connection.streams.get_mut(&stream_id) {
                                    req.set_terminal_overload_reason(Some(
                                        OverloadShedReason::RequestBufferCap,
                                    ));
                                    req.mark_terminal_outcome_recorded();
                                    terminalize_stream(
                                        req,
                                        TerminalReason::Rejected(RejectionReason::Overloaded),
                                        &metrics,
                                    );
                                }
                                connection.streams.remove(&stream_id);
                                break;
                            }
                        }
                        Err(quiche::h3::Error::Done) => break,
                        Err(err) => {
                            let rid = connection.streams.get(&stream_id).map(|r| r.request_id);
                            error!(
                                "request_id={} HTTP/3 recv_body protocol error on stream {}: {:?}",
                                rid.map_or_else(|| "-".to_string(), |id| id.to_string()),
                                stream_id,
                                err
                            );
                            if let Some(req) = connection.streams.get(&stream_id) {
                                let _ = observe_proxy_error_outcome(
                                    &metrics,
                                    RouteOutcomeTarget {
                                        route: req.upstream_name.as_deref().unwrap_or("unrouted"),
                                    },
                                    Some(BackendOutcomeTarget {
                                        upstream: req
                                            .upstream_name
                                            .as_deref()
                                            .unwrap_or("unrouted"),
                                        backend_addr: req.backend_addr.as_deref(),
                                        backend_index: req.backend_index,
                                    }),
                                    req.start.elapsed(),
                                    Some(http::StatusCode::BAD_GATEWAY),
                                    &ProxyError::Protocol(format!(
                                        "recv_body protocol error on stream {}",
                                        stream_id
                                    )),
                                    None,
                                );
                                resilience
                                    .adaptive_admission
                                    .observe(req.start.elapsed(), true);
                            }
                            if let Some(req) = connection.streams.get_mut(&stream_id) {
                                terminalize_stream(
                                    req,
                                    TerminalReason::Rejected(RejectionReason::ValidationFailed),
                                    &metrics,
                                );
                            }
                            connection.streams.remove(&stream_id);
                            let _ = Self::send_simple_response(
                                h3,
                                &mut connection.quic,
                                stream_id,
                                http::StatusCode::BAD_REQUEST,
                                b"malformed request stream\n",
                            );
                            break;
                        }
                    }
                },
                Ok((stream_id, quiche::h3::Event::Finished)) => {
                    if let Some(req) = connection.streams.get_mut(&stream_id) {
                        req.transition_request_body_finished();
                        let _ = Self::flush_request_buffer(req, &metrics);
                        // Upstream polling and response dispatch are handled entirely
                        // by advance_streams_non_blocking, called unconditionally below.
                    }
                }
                Ok((stream_id, quiche::h3::Event::Reset(error_code))) => {
                    if let Some(req) = connection.streams.get_mut(&stream_id) {
                        let phase = terminalize_stream(
                            req,
                            TerminalReason::Cancelled(CancellationReason::ClientReset),
                            &metrics,
                        );
                        debug!(
                            "stream {} reset by client (error_code={}, phase={:?}): resources released",
                            stream_id, error_code, phase
                        );
                    }
                    connection.streams.remove(&stream_id);
                }
                Ok((_stream_id, quiche::h3::Event::PriorityUpdate)) => {}
                Ok((_stream_id, quiche::h3::Event::GoAway)) => {}
                Err(quiche::h3::Error::Done) => break,
                Err(e) => return Err(e),
            }
        }

        Self::advance_streams_non_blocking(
            &mut connection.streams,
            &mut connection.quic,
            h3,
            &exec_ctx,
            &shared_ctx,
            &progress_config,
        )?;

        Ok(())
    }

    pub(crate) fn resolve_scoped_rate_limit_key(
        rule: &crate::resilience::scoped_rate_limit::ScopedRateLimitRule,
        route: &str,
        method: &str,
        path: &str,
        authority: Option<&str>,
        client_addr: SocketAddr,
        header_lookup: Option<&LbHeaderLookup<'_>>,
    ) -> Option<String> {
        match rule.scope() {
            ScopedRateLimitScope::Route => Some(route.to_string()),
            ScopedRateLimitScope::Client => Some(
                Self::resolve_lb_key(
                    "",
                    Some(rule.key_spec().unwrap_or("peer_ip")),
                    method,
                    path,
                    authority,
                    None,
                    Some(client_addr),
                    header_lookup,
                )
                .value,
            ),
            ScopedRateLimitScope::Tenant => rule.key_spec().map(|key_spec| {
                Self::resolve_lb_key(
                    "",
                    Some(key_spec),
                    method,
                    path,
                    authority,
                    None,
                    Some(client_addr),
                    header_lookup,
                )
                .value
            }),
            ScopedRateLimitScope::Token => Some(
                Self::resolve_lb_key(
                    "",
                    Some(rule.key_spec().unwrap_or("bearer_token")),
                    method,
                    path,
                    authority,
                    None,
                    Some(client_addr),
                    header_lookup,
                )
                .value,
            ),
        }
    }
}

#[cfg(test)]
mod tests {

    use std::time::UNIX_EPOCH;

    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use hmac::{Hmac, Mac};
    use serde_json::Value;
    use sha2::Sha256;
    use spooky_config::{
        config::{ScopedRateLimit, ScopedRateLimitScope},
        runtime::{RuntimeApiKeyAuth, RuntimeAuthPolicy, RuntimeJwtAuth, RuntimeUpstreamPolicy},
    };

    use super::{auth::append_auth_request_headers, *};
    use crate::runtime::connection::auth::PendingHeaderMutation;

    fn sample_pending_forward(headers: Vec<quiche::h3::Header>) -> PendingForward {
        PendingForward {
            path: Arc::<str>::from("/"),
            authority: Some(Arc::<str>::from("example.com")),
            route_path_len: 1,
            route_host_specific: false,
            backend_addr: Arc::<str>::from("http://127.0.0.1:8080"),
            client_addr: "127.0.0.1:443".parse().expect("client addr"),
            request_id: 7,
            ..PendingForward::sample_for_test(headers)
        }
    }

    fn test_hs256_jwt(secret: &str, claims: serde_json::Value, alg: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({ "alg": alg, "typ": "JWT" }))
                .expect("serialize header"),
        );
        let payload =
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("serialize claims"));
        let signing_input = format!("{header}.{payload}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("mac");
        mac.update(signing_input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{signing_input}.{signature}")
    }

    #[test]
    fn internal_pool_errors_are_classified_as_control_plane_only() {
        assert!(QUICListener::is_internal_pool_control_error(
            &PoolError::InflightLimiterClosed
        ));
        assert!(QUICListener::is_internal_pool_control_error(
            &PoolError::UnknownBackend("missing".to_string())
        ));
    }

    #[test]
    fn backend_overload_is_not_classified_as_internal_pool_error() {
        assert!(!QUICListener::is_internal_pool_control_error(
            &PoolError::BackendOverloaded("busy".to_string())
        ));
    }

    #[test]
    fn circuit_open_is_not_classified_as_internal_pool_error() {
        assert!(!QUICListener::is_internal_pool_control_error(
            &PoolError::CircuitOpen("open".to_string())
        ));
    }

    #[test]
    fn send_connect_error_with_tls_details_maps_to_tls_health_failure() {
        assert_eq!(
            spooky_errors::classify_upstream_error_detail(
                "client error (Connect): tls handshake failed: invalid certificate",
                true,
            ),
            spooky_errors::UpstreamErrorClassification::tls(
                spooky_errors::UpstreamTlsReason::Handshake,
            )
        );
    }

    #[test]
    fn send_connect_error_without_tls_details_maps_to_transport_health_failure() {
        assert_eq!(
            spooky_errors::classify_upstream_error_detail(
                "client error (Connect): connection refused",
                true,
            ),
            spooky_errors::UpstreamErrorClassification::transport()
        );
    }

    #[test]
    fn send_error_with_timeout_detail_maps_to_timeout_health_failure() {
        assert_eq!(
            spooky_errors::classify_upstream_error_detail("request timed out", false),
            spooky_errors::UpstreamErrorClassification::timeout()
        );
    }

    #[test]
    fn backend_failure_reason_for_proxy_error_preserves_transport_protocol_and_bridge_buckets() {
        assert_eq!(
            backend_failure_reason_for_proxy_error(&ProxyError::Transport(
                "connection reset".to_string()
            )),
            BackendFailureReason::UpstreamTransport
        );
        assert_eq!(
            backend_failure_reason_for_proxy_error(&ProxyError::Pool(PoolError::UnknownBackend(
                "missing".to_string()
            ))),
            BackendFailureReason::UpstreamTransport
        );
        assert_eq!(
            backend_failure_reason_for_proxy_error(&ProxyError::Protocol(
                "bad response frame".to_string()
            )),
            BackendFailureReason::UpstreamProtocol
        );
        assert_eq!(
            backend_failure_reason_for_proxy_error(&ProxyError::Bridge(
                spooky_errors::BridgeError::InvalidHeader
            )),
            BackendFailureReason::UpstreamBridge
        );
    }

    #[test]
    fn api_key_authorization_requires_exact_configured_match() {
        let policy = RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy {
                api_key: Some(RuntimeApiKeyAuth {
                    header_name: "x-api-key".to_string(),
                    keys: vec!["secret-key".to_string()],
                }),
                jwt: None,
                external_auth: None,
                required_scopes: Vec::new(),
                required_roles: Vec::new(),
            },
            host: Default::default(),
            forwarded_headers: Default::default(),
            protocol: Default::default(),
        };
        let headers = [("x-api-key".to_string(), "secret-key".to_string())]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();
        let wrong_lookup = |_: &str| Some("wrong-key".to_string());

        assert!(super::super::admission::api_key_is_authorized(
            &policy,
            Some(&lookup)
        ));
        assert!(!super::super::admission::api_key_is_authorized(
            &policy,
            Some(&wrong_lookup)
        ));
        assert!(!super::super::admission::api_key_is_authorized(
            &policy, None
        ));
    }

    #[test]
    fn hs256_jwt_validation_enforces_signature_and_claims() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let token = test_hs256_jwt(
            "jwt-secret",
            serde_json::json!({
                "sub": "user-1",
                "iss": "issuer-1",
                "aud": "aud-1",
                "exp": 4_000_000_000u64,
                "nbf": 1_699_999_900u64,
            }),
            "HS256",
        );
        let policy = RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy {
                api_key: None,
                jwt: Some(RuntimeJwtAuth {
                    secret: "jwt-secret".to_string(),
                    issuer: Some("issuer-1".to_string()),
                    audience: Some("aud-1".to_string()),
                    clock_skew: Duration::from_secs(30),
                    ..RuntimeJwtAuth::default()
                }),
                external_auth: None,
                required_scopes: Vec::new(),
                required_roles: Vec::new(),
            },
            host: Default::default(),
            forwarded_headers: Default::default(),
            protocol: Default::default(),
        };
        let headers = [("authorization".to_string(), format!("Bearer {token}"))]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();

        assert!(super::super::admission::jwt_is_authorized(
            &policy,
            Some(&lookup)
        ));
        assert!(
            super::super::admission::validated_hs256_jwt_claims(
                token.as_str(),
                policy.upstream_auth.jwt.as_ref().expect("jwt policy"),
                now
            )
            .is_some()
        );

        let wrong_secret = RuntimeJwtAuth {
            secret: "wrong".to_string(),
            issuer: Some("issuer-1".to_string()),
            audience: Some("aud-1".to_string()),
            clock_skew: Duration::from_secs(30),
            ..RuntimeJwtAuth::default()
        };
        assert!(
            super::super::admission::validated_hs256_jwt_claims(token.as_str(), &wrong_secret, now)
                .is_none()
        );

        let expired = test_hs256_jwt(
            "jwt-secret",
            serde_json::json!({ "exp": 1_699_999_900u64 }),
            "HS256",
        );
        assert!(
            super::super::admission::validated_hs256_jwt_claims(
                expired.as_str(),
                &RuntimeJwtAuth {
                    secret: "jwt-secret".to_string(),
                    issuer: None,
                    audience: None,
                    clock_skew: Duration::ZERO,
                    ..RuntimeJwtAuth::default()
                },
                now
            )
            .is_none()
        );

        let validated = super::super::admission::validate_jwt_token(
            token.as_str(),
            policy.upstream_auth.jwt.as_ref().expect("jwt policy"),
            now,
        )
        .expect("validated jwt");
        assert_eq!(
            validated.algorithm,
            spooky_config::config::JwtAlgorithm::Hs256
        );
        assert_eq!(
            validated.claims.get("sub").and_then(Value::as_str),
            Some("user-1")
        );
    }

    #[test]
    fn jwt_validation_rejects_algorithms_by_policy_with_stable_reasons() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let token = test_hs256_jwt(
            "jwt-secret",
            serde_json::json!({
                "iss": "issuer-1",
                "aud": "aud-1",
                "exp": 4_000_000_000u64,
            }),
            "HS256",
        );

        let failure = super::super::admission::validate_jwt_token(
            token.as_str(),
            &RuntimeJwtAuth {
                secret: "jwt-secret".to_string(),
                issuer: Some("issuer-1".to_string()),
                audience: Some("aud-1".to_string()),
                allowed_algorithms: vec![spooky_config::config::JwtAlgorithm::Rs256],
                clock_skew: Duration::from_secs(30),
                ..RuntimeJwtAuth::default()
            },
            now,
        )
        .expect_err("algorithm must be rejected");

        assert_eq!(failure.reason.as_str(), "algorithm_not_allowed");
    }

    #[test]
    fn jwt_validation_rejects_missing_required_kid_with_stable_reason() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let token = test_hs256_jwt(
            "jwt-secret",
            serde_json::json!({
                "iss": "issuer-1",
                "aud": "aud-1",
                "exp": 4_000_000_000u64,
            }),
            "HS256",
        );

        let failure = super::super::admission::validate_jwt_token(
            token.as_str(),
            &RuntimeJwtAuth {
                secret: "jwt-secret".to_string(),
                issuer: Some("issuer-1".to_string()),
                audience: Some("aud-1".to_string()),
                require_kid: true,
                clock_skew: Duration::from_secs(30),
                ..RuntimeJwtAuth::default()
            },
            now,
        )
        .expect_err("kid must be required");

        assert_eq!(failure.reason.as_str(), "missing_kid");
    }

    #[test]
    fn jwt_rbac_requires_configured_scopes_and_roles() {
        let policy = RuntimeUpstreamPolicy {
            upstream_auth: RuntimeAuthPolicy {
                api_key: None,
                jwt: None,
                external_auth: None,
                required_scopes: vec!["read:fast".to_string()],
                required_roles: vec!["admin".to_string()],
            },
            host: Default::default(),
            forwarded_headers: Default::default(),
            protocol: Default::default(),
        };
        let allowed_claims = serde_json::json!({
            "scope": "read:fast write:slow",
            "roles": ["admin", "operator"]
        });
        let denied_claims = serde_json::json!({
            "scope": "write:slow",
            "roles": ["operator"]
        });

        assert!(super::super::admission::jwt_claims_satisfy_rbac(
            &policy,
            &allowed_claims
        ));
        assert!(!super::super::admission::jwt_claims_satisfy_rbac(
            &policy,
            &denied_claims
        ));
    }

    #[test]
    fn resolve_lb_key_supports_peer_ip_and_bearer_token() {
        let headers = [("authorization".to_string(), "Bearer token-1".to_string())]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();
        let client_addr = "203.0.113.9:443".parse().expect("client addr");

        assert_eq!(
            QUICListener::resolve_lb_key(
                "",
                Some("peer_ip"),
                "GET",
                "/",
                Some("api.example.com"),
                None,
                Some(client_addr),
                Some(&lookup),
            )
            .value,
            "203.0.113.9"
        );
        assert_eq!(
            QUICListener::resolve_lb_key(
                "",
                Some("bearer_token"),
                "GET",
                "/",
                Some("api.example.com"),
                None,
                Some(client_addr),
                Some(&lookup),
            )
            .value,
            "token-1"
        );
    }

    #[test]
    fn canonical_lb_key_resolver_extracts_common_sources() {
        let headers = [
            ("authorization".to_string(), "Bearer token-3".to_string()),
            ("cookie".to_string(), "session=s123; theme=dark".to_string()),
            ("x-user-id".to_string(), "alice".to_string()),
        ]
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();
        let client_addr = "203.0.113.9:443".parse().expect("client addr");

        let assert_key = |spec: &str, expected: &str| {
            let resolved = QUICListener::resolve_lb_key(
                "",
                Some(spec),
                "POST",
                "/api/items?tenant=acme",
                Some("api.example.com"),
                Some("cid-123"),
                Some(client_addr),
                Some(&lookup),
            );
            assert_eq!(resolved.value, expected);
            assert!(matches!(
                resolved.source,
                super::lb_key::LbKeySource::ConfiguredSpec
            ));
        };

        assert_key("header:x-user-id", "alice");
        assert_key("cookie:session", "s123");
        assert_key("query:tenant", "acme");
        assert_key("bearer_token", "token-3");
        assert_key("cid", "cid-123");
        assert_key("peer_ip", "203.0.113.9");
        assert_key("path", "/api/items");
        assert_key("authority", "api.example.com");
        assert_key("method", "POST");
    }

    #[test]
    fn canonical_lb_key_resolver_unifies_default_and_sticky_cid_fallbacks() {
        let sticky = QUICListener::resolve_lb_key(
            "sticky-cid",
            Some("header:x-user-id"),
            "GET",
            "/resource",
            Some("api.example.com"),
            Some("cid-123"),
            None,
            None,
        );
        assert_eq!(sticky.value, "cid-123");
        assert!(matches!(
            sticky.source,
            super::lb_key::LbKeySource::StickyCidFallback
        ));

        let authority_default = QUICListener::resolve_lb_key(
            "",
            Some("header:x-user-id"),
            "GET",
            "/resource",
            Some("api.example.com"),
            None,
            None,
            None,
        );
        assert_eq!(authority_default.value, "api.example.com");
        assert!(matches!(
            authority_default.source,
            super::lb_key::LbKeySource::DefaultFallback
        ));

        let path_default = QUICListener::resolve_lb_key(
            "",
            Some("header:x-user-id"),
            "GET",
            "/resource",
            None,
            None,
            None,
            None,
        );
        assert_eq!(path_default.value, "/resource");

        let method_default = QUICListener::resolve_lb_key(
            "",
            Some("header:x-user-id"),
            "GET",
            "",
            None,
            None,
            None,
            None,
        );
        assert_eq!(method_default.value, "GET");
        assert!(matches!(
            method_default.source,
            super::lb_key::LbKeySource::DefaultFallback
        ));
    }

    #[test]
    fn canonical_lb_key_route_request_adapter_matches_direct_resolver() {
        let headers = [
            ("authorization".to_string(), "Bearer token-4".to_string()),
            ("x-user-id".to_string(), "bob".to_string()),
        ]
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();
        let route_request = TestTargetResolutionRequest::new(
            "GET",
            "/api/items?tenant=acme",
            Some("api.example.com"),
            Some("cid-456"),
            Some(&lookup),
        );

        let direct_header = QUICListener::resolve_lb_key(
            "",
            Some("header:x-user-id"),
            "GET",
            "/api/items?tenant=acme",
            Some("api.example.com"),
            Some("cid-456"),
            None,
            Some(&lookup),
        );
        let routed_header = QUICListener::resolve_lb_key_for_runtime_request(
            spooky_config::runtime::RuntimeLoadBalancingStrategy::RoundRobin,
            Some(&spooky_config::runtime::RuntimeRequestKeySpec::Header(
                "x-user-id".to_string(),
            )),
            &route_request,
        );
        assert_eq!(direct_header.value, routed_header.value);
        assert!(matches!(
            routed_header.source,
            super::lb_key::LbKeySource::ConfiguredSpec
        ));

        let direct_sticky = QUICListener::resolve_lb_key(
            "sticky-cid",
            Some("header:x-missing"),
            "GET",
            "/api/items?tenant=acme",
            Some("api.example.com"),
            Some("cid-456"),
            None,
            Some(&lookup),
        );
        let routed_sticky = QUICListener::resolve_lb_key_for_runtime_request(
            spooky_config::runtime::RuntimeLoadBalancingStrategy::StickyCid,
            Some(&spooky_config::runtime::RuntimeRequestKeySpec::Header(
                "x-missing".to_string(),
            )),
            &route_request,
        );
        assert_eq!(direct_sticky.value, routed_sticky.value);
        assert!(matches!(
            routed_sticky.source,
            super::lb_key::LbKeySource::StickyCidFallback
        ));
    }

    #[test]
    fn resolve_scoped_rate_limit_key_defaults_match_scope() {
        let client_rule = crate::resilience::scoped_rate_limit::ScopedRateLimitRule::from_config(
            &ScopedRateLimit {
                name: "client".to_string(),
                scope: ScopedRateLimitScope::Client,
                requests_per_sec: 10,
                burst: 10,
                key: None,
                route_allowlist: Vec::new(),
                idle_ttl_secs: 300,
            },
        );
        let token_rule = crate::resilience::scoped_rate_limit::ScopedRateLimitRule::from_config(
            &ScopedRateLimit {
                name: "token".to_string(),
                scope: ScopedRateLimitScope::Token,
                requests_per_sec: 10,
                burst: 10,
                key: None,
                route_allowlist: Vec::new(),
                idle_ttl_secs: 300,
            },
        );
        let headers = [("authorization".to_string(), "Bearer token-2".to_string())]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();
        let client_addr = "198.51.100.10:443".parse().expect("client addr");

        assert_eq!(
            QUICListener::resolve_scoped_rate_limit_key(
                &client_rule,
                "api",
                "GET",
                "/resource",
                Some("api.example.com"),
                client_addr,
                Some(&lookup),
            )
            .as_deref(),
            Some("198.51.100.10")
        );
        assert_eq!(
            QUICListener::resolve_scoped_rate_limit_key(
                &token_rule,
                "api",
                "GET",
                "/resource",
                Some("api.example.com"),
                client_addr,
                Some(&lookup),
            )
            .as_deref(),
            Some("token-2")
        );
    }

    #[test]
    fn resolve_scoped_rate_limit_key_falls_back_to_default_request_key() {
        let tenant_rule = crate::resilience::scoped_rate_limit::ScopedRateLimitRule::from_config(
            &ScopedRateLimit {
                name: "tenant".to_string(),
                scope: ScopedRateLimitScope::Tenant,
                requests_per_sec: 10,
                burst: 10,
                key: Some("header:x-tenant-id".to_string()),
                route_allowlist: Vec::new(),
                idle_ttl_secs: 300,
            },
        );
        let headers = std::collections::HashMap::<String, String>::new();
        let lookup = |name: &str| headers.get(&name.to_ascii_lowercase()).cloned();
        let client_addr = "198.51.100.10:443".parse().expect("client addr");

        assert_eq!(
            QUICListener::resolve_scoped_rate_limit_key(
                &tenant_rule,
                "api",
                "GET",
                "/resource",
                Some("api.example.com"),
                client_addr,
                Some(&lookup),
            )
            .as_deref(),
            Some("api.example.com")
        );
    }

    #[test]
    fn resolve_lb_key_uses_sticky_cid_before_default_fallback() {
        let resolved = QUICListener::resolve_lb_key(
            "sticky-cid",
            Some("header:x-user-id"),
            "GET",
            "/resource",
            Some("api.example.com"),
            Some("cid-123"),
            None,
            None,
        );

        assert_eq!(resolved.value, "cid-123");
        assert!(matches!(
            resolved.source,
            super::lb_key::LbKeySource::StickyCidFallback
        ));
    }

    #[test]
    fn pending_forward_request_headers_apply_auth_mutations() {
        let pending_forward = PendingForward {
            auth_header_mutations: vec![
                PendingHeaderMutation::Upsert {
                    name: b"x-user-id".to_vec(),
                    value: b"fresh".to_vec(),
                },
                PendingHeaderMutation::Remove {
                    name: b"x-remove-me".to_vec(),
                },
            ],
            ..sample_pending_forward(vec![
                quiche::h3::Header::new(b":method", b"GET"),
                quiche::h3::Header::new(b"x-user-id", b"stale"),
                quiche::h3::Header::new(b"x-remove-me", b"1"),
            ])
        };

        let headers = pending_forward.request_headers();
        assert!(headers.iter().any(|header| header.name() == b":method"));
        assert!(
            headers
                .iter()
                .any(|header| header.name() == b"x-user-id" && header.value() == b"fresh")
        );
        assert!(
            !headers
                .iter()
                .any(|header| header.name() == b"x-user-id" && header.value() == b"stale")
        );
        assert!(!headers.iter().any(|header| header.name() == b"x-remove-me"));
    }

    #[test]
    fn append_auth_request_headers_strips_hop_by_hop_and_framing_headers() {
        let pending_forward = sample_pending_forward(vec![
            quiche::h3::Header::new(b":method", b"GET"),
            quiche::h3::Header::new(b"host", b"client.example.com"),
            quiche::h3::Header::new(b"content-length", b"42"),
            quiche::h3::Header::new(b"connection", b"keep-alive"),
            quiche::h3::Header::new(b"cookie", b"session=1"),
            quiche::h3::Header::new(b"authorization", b"Bearer token"),
        ]);
        let mut builder = Request::builder()
            .method(http::Method::GET)
            .uri("https://auth.internal/check");

        append_auth_request_headers(&mut builder, &pending_forward, &[]);

        let headers = builder.headers_ref().expect("headers");
        assert!(!headers.contains_key(http::header::HOST));
        assert!(!headers.contains_key(http::header::CONTENT_LENGTH));
        assert!(!headers.contains_key(http::header::CONNECTION));
        assert_eq!(
            headers
                .get(http::header::COOKIE)
                .and_then(|value| value.to_str().ok()),
            Some("session=1")
        );
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer token")
        );
    }
}
