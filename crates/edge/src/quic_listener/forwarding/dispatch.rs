use spooky_errors::{
    HedgeOutcomeTelemetryReason, HedgePolicyDecision, HedgePolicyFacts, HedgePrimaryState,
    RetryPolicyDecision, RetryPolicyFacts, evaluate_hedge_policy, evaluate_retry_policy,
    is_idempotent_method,
};
use spooky_lb::alternate_backend::{
    AlternateBackendDecision, AlternateBackendFailureReason, choose_alternate_backend,
};

use super::*;
use crate::observability::{HedgeDecisionReason, RetryDecisionReason};
use crate::runtime::connection::response::ForwardingPolicyTelemetry;

const MAX_UPSTREAM_RETRY_ATTEMPTS: u8 = 1;
type UpstreamRequest = Request<BoxBody<Bytes, Infallible>>;

#[derive(Clone, Debug)]
enum ResolvedAlternateBackend {
    Selected {
        address: String,
    },
    Unavailable {
        reason: AlternateBackendFailureReason,
    },
}

impl ResolvedAlternateBackend {
    fn is_available(&self) -> bool {
        matches!(self, Self::Selected { .. })
    }

    fn failure_reason(&self) -> Option<AlternateBackendFailureReason> {
        match self {
            Self::Selected { .. } => None,
            Self::Unavailable { reason } => Some(*reason),
        }
    }
}

struct AlternateBodylessCandidate {
    backend: String,
    request: UpstreamRequest,
}

struct RetryExecutionCtx<'a> {
    request_id: u64,
    route_name: &'a str,
    policy: ForwardingRetryHedgePolicy,
    policy_telemetry: &'a mut ForwardingPolicyTelemetry,
    retry_budget: &'a crate::resilience::retry_budget::RetryBudget,
    alternate_backend: Option<&'a ResolvedAlternateBackend>,
    backend_endpoints: &'a HashMap<String, BackendEndpoint>,
    pending_forward: &'a PendingForward,
    circuit_breakers: Arc<crate::resilience::circuit_breaker::CircuitBreakers>,
    transport: Arc<UpstreamTransportPool>,
}

#[derive(Clone, Copy)]
struct ForwardingRetryHedgePolicy {
    method_idempotent: bool,
    bodyless_mode: bool,
    hedge_method_allowed: bool,
    hedge_configured: bool,
    hedge_tunnel_request: bool,
}

impl ForwardingRetryHedgePolicy {
    fn new(
        method_idempotent: bool,
        bodyless_mode: bool,
        hedge_method_allowed: bool,
        hedge_configured: bool,
        hedge_tunnel_request: bool,
    ) -> Self {
        Self {
            method_idempotent,
            bodyless_mode,
            hedge_method_allowed,
            hedge_configured,
            hedge_tunnel_request,
        }
    }

    fn hedge_before_delay(
        self,
        alternate_backend: Option<&ResolvedAlternateBackend>,
    ) -> HedgePolicyDecision {
        let (alternate_backend_available, alternate_backend_failure) =
            alternate_backend_policy_state(alternate_backend);
        evaluate_hedge_policy(HedgePolicyFacts {
            hedging_configured: self.hedge_configured,
            method_allowed: self.hedge_method_allowed,
            request_body_replayable: self.bodyless_mode,
            tunnel_request: self.hedge_tunnel_request,
            alternate_backend_available,
            alternate_backend_failure,
            budget_available: false,
            primary_state: HedgePrimaryState::InFlightBeforeDelay,
        })
    }

    fn hedge_after_delay(self, budget_available: bool) -> HedgePolicyDecision {
        evaluate_hedge_policy(HedgePolicyFacts {
            hedging_configured: self.hedge_configured,
            method_allowed: self.hedge_method_allowed,
            request_body_replayable: self.bodyless_mode,
            tunnel_request: self.hedge_tunnel_request,
            alternate_backend_available: true,
            alternate_backend_failure: None,
            budget_available,
            primary_state: HedgePrimaryState::InFlightAfterDelay,
        })
    }

    fn retry_after_error(
        self,
        primary_err: &ProxyError,
        retry_count: u8,
        max_attempts: u8,
        budget_available: bool,
        alternate_backend: Option<&ResolvedAlternateBackend>,
    ) -> RetryPolicyDecision {
        let (alternate_backend_available, alternate_backend_failure) =
            alternate_backend_policy_state(alternate_backend);
        evaluate_retry_policy(RetryPolicyFacts {
            retryability: spooky_errors::classify_retryability(primary_err),
            method_idempotent: self.method_idempotent,
            request_body_replayable: self.bodyless_mode,
            attempt_count: retry_count,
            max_attempts,
            budget_available,
            alternate_backend_available,
            alternate_backend_failure,
        })
    }
}

fn alternate_backend_policy_state(
    alternate_backend: Option<&ResolvedAlternateBackend>,
) -> (bool, Option<AlternateBackendFailureReason>) {
    (
        alternate_backend.is_some_and(ResolvedAlternateBackend::is_available),
        alternate_backend.and_then(ResolvedAlternateBackend::failure_reason),
    )
}

fn retry_budget_available_for_error(
    primary_err: &ProxyError,
    route_name: &str,
    retry_budget: &crate::resilience::retry_budget::RetryBudget,
) -> bool {
    matches!(primary_err, ProxyError::Pool(PoolError::CircuitOpen(_)))
        || retry_budget.allow_retry(route_name).is_ok()
}

impl QUICListener {
    async fn send_upstream_request(
        backend: String,
        request: UpstreamRequest,
        circuit_breakers: Arc<crate::resilience::circuit_breaker::CircuitBreakers>,
        transport: Arc<UpstreamTransportPool>,
    ) -> Result<Response<Incoming>, ProxyError> {
        if !circuit_breakers.allow_request(&backend) {
            return Err(ProxyError::Pool(PoolError::CircuitOpen(backend)));
        }

        let send_result = transport.send_backend_request(&backend, request).await;
        match &send_result {
            Ok(_) => circuit_breakers.record_success(&backend),
            _ => circuit_breakers.record_failure(&backend),
        }
        send_result
    }

    #[allow(clippy::too_many_arguments)]
    async fn forward_http1_websocket_tunnel(
        endpoint: BackendEndpoint,
        pending_forward: Arc<PendingForward>,
        mut body_rx: mpsc::Receiver<Bytes>,
        backend_timeout: Duration,
        metrics: Arc<Metrics>,
    ) -> ForwardResult {
        let request = pending_forward.build_http1_websocket_tunnel_request(&endpoint)?;

        let stream = tokio::time::timeout(
            backend_timeout,
            tokio::net::TcpStream::connect(endpoint.authority()),
        )
        .await
        .map_err(|_| ProxyError::Timeout)?
        .map_err(|err| ProxyError::Transport(err.to_string()))?;
        let resolved_addr = stream
            .peer_addr()
            .map_err(|err| ProxyError::Transport(err.to_string()))?;
        metrics.record_backend_connect(
            endpoint.authority(),
            endpoint.authority_host(),
            resolved_addr,
        );
        let io = TokioIo::new(stream);
        let (mut sender, conn) = client_http1::handshake(io)
            .await
            .map_err(|err| ProxyError::Transport(err.to_string()))?;
        tokio::spawn(async move {
            let _ = conn.with_upgrades().await;
        });

        let mut response = tokio::time::timeout(backend_timeout, sender.send_request(request))
            .await
            .map_err(|_| ProxyError::Timeout)?
            .map_err(|err| ProxyError::Transport(err.to_string()))?;

        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            let status = response.status();
            let headers = response.headers().clone();
            return Ok(ForwardSuccess::Response {
                status,
                headers,
                body: response.into_body(),
            });
        }

        let upgraded = upgrade::on(&mut response);
        let headers = response.headers().clone();
        let (chunk_tx, chunk_rx) = mpsc::channel(RESPONSE_CHUNK_CHANNEL_CAPACITY);
        let fut = async move {
            let upgraded = match upgraded.await {
                Ok(upgraded) => upgraded,
                Err(err) => {
                    let _ = chunk_tx
                        .send(ResponseChunk::Error(ProxyError::Transport(err.to_string())))
                        .await;
                    return;
                }
            };
            let io = TokioIo::new(upgraded);
            let (mut reader, mut writer) = tokio::io::split(io);
            let write_fut = async {
                while let Some(chunk) = body_rx.recv().await {
                    writer.write_all(&chunk).await?;
                }
                writer.shutdown().await
            };
            let read_fut = async {
                let mut buf = [0u8; RESPONSE_CHUNK_BYTES_LIMIT];
                loop {
                    match reader.read(&mut buf).await {
                        Ok(0) => return Ok::<(), std::io::Error>(()),
                        Ok(read) => {
                            if chunk_tx
                                .send(ResponseChunk::Data(Bytes::copy_from_slice(&buf[..read])))
                                .await
                                .is_err()
                            {
                                return Ok(());
                            }
                        }
                        Err(err) => return Err(err),
                    }
                }
            };
            match tokio::try_join!(write_fut, read_fut) {
                Ok(((), ())) => {
                    let _ = chunk_tx.send(ResponseChunk::End).await;
                }
                Err(err) => {
                    let _ = chunk_tx
                        .send(ResponseChunk::Error(ProxyError::Transport(err.to_string())))
                        .await;
                }
            }
        };
        let _ = spawn_async_task(fut, "ws-h1-tunnel");

        Ok(ForwardSuccess::Tunnel {
            status: StatusCode::OK,
            headers,
            response_chunk_rx: chunk_rx,
        })
    }

    fn resolve_alternate_backend(
        upstream_pool: &Arc<RwLock<UpstreamPool>>,
        primary_index: usize,
    ) -> ResolvedAlternateBackend {
        let Ok(pool) = upstream_pool.read() else {
            return ResolvedAlternateBackend::Unavailable {
                reason: AlternateBackendFailureReason::PoolUnavailable,
            };
        };
        match choose_alternate_backend(&pool, &[primary_index], None) {
            AlternateBackendDecision::Select(choice) => {
                if let Some(address) = pool.backend_address(choice.index) {
                    ResolvedAlternateBackend::Selected {
                        address: address.to_string(),
                    }
                } else {
                    ResolvedAlternateBackend::Unavailable {
                        reason: AlternateBackendFailureReason::BackendAddressMissing,
                    }
                }
            }
            AlternateBackendDecision::DoNotSelect { denial } => {
                ResolvedAlternateBackend::Unavailable { reason: denial }
            }
        }
    }

    fn build_alternate_bodyless_candidate(
        alternate_backend: Option<&ResolvedAlternateBackend>,
        backend_endpoints: &HashMap<String, BackendEndpoint>,
        pending_forward: &PendingForward,
    ) -> Option<AlternateBodylessCandidate> {
        match alternate_backend? {
            ResolvedAlternateBackend::Selected { address } => {
                let endpoint = backend_endpoints.get(address)?;
                let request = pending_forward.build_bodyless_request(endpoint).ok()?;
                Some(AlternateBodylessCandidate {
                    backend: address.clone(),
                    request,
                })
            }
            ResolvedAlternateBackend::Unavailable { .. } => None,
        }
    }

    async fn retry_primary_error(
        primary_err: ProxyError,
        retry_ctx: RetryExecutionCtx<'_>,
    ) -> Result<Response<Incoming>, ProxyError> {
        let RetryExecutionCtx {
            request_id,
            route_name,
            policy,
            policy_telemetry,
            retry_budget,
            alternate_backend,
            backend_endpoints,
            pending_forward,
            circuit_breakers,
            transport,
        } = retry_ctx;
        let retry_decision = policy.retry_after_error(
            &primary_err,
            policy_telemetry.retry.count,
            MAX_UPSTREAM_RETRY_ATTEMPTS,
            retry_budget_available_for_error(&primary_err, route_name, retry_budget),
            alternate_backend,
        );
        let (retry_reason, canonical_retry_reason) = match retry_decision {
            RetryPolicyDecision::Retry { reason } => (reason.into(), RetryDecisionReason::from(reason)),
            RetryPolicyDecision::DoNotRetry { denial } => {
                policy_telemetry.retry.record_denial(denial);
                // Phase 6: canonical operational reason (upstream=, decision=retry)
                // with the raw denial kept as debug-only detail.
                let reason = denial
                    .map(RetryDecisionReason::from)
                    .unwrap_or(RetryDecisionReason::RetryPolicyDisabled);
                debug!(
                    "retry decision: request_id={} upstream={} decision=denied reason={} detail={:?}",
                    request_id,
                    route_name,
                    reason.slug(),
                    denial
                );
                return Err(primary_err);
            }
        };

        let Some(AlternateBodylessCandidate {
            backend: retry_backend,
            request: retry_request,
        }) = Self::build_alternate_bodyless_candidate(
            alternate_backend,
            backend_endpoints,
            pending_forward,
        )
        else {
            return Err(primary_err);
        };

        policy_telemetry.retry.record_attempt(retry_reason);
        info!(
            "retry decision: request_id={} upstream={} decision=retry reason={} backend={}",
            request_id,
            route_name,
            canonical_retry_reason.slug(),
            retry_backend
        );
        Self::send_upstream_request(retry_backend, retry_request, circuit_breakers, transport).await
    }

    pub(super) fn spawn_upstream_forward_task(
        req: &RequestEnvelope,
        pending_forward: Arc<PendingForward>,
        backend_endpoint: BackendEndpoint,
        request: Option<UpstreamRequest>,
        websocket_tunnel_body_rx: Option<mpsc::Receiver<Bytes>>,
        exec_ctx: &ForwardingExecutionCtx<'_>,
        shared_ctx: &ForwardingSharedCtx<'_>,
    ) -> Result<oneshot::Receiver<UpstreamResult>, ProxyError> {
        let metrics = Arc::clone(&shared_ctx.metrics);
        let resilience = shared_ctx.resilience;
        let fwd_addr = pending_forward.backend_addr.to_string();
        let cb = Arc::clone(&resilience.circuit_breakers);
        let retry_budget = Arc::clone(&resilience.retry_budget);
        let route_name = pending_forward.upstream_name.to_string();
        let backend_timeout = exec_ctx.backend_timeout;
        let backend_endpoints = Arc::clone(&exec_ctx.backend_endpoints);
        let transport = Arc::clone(&exec_ctx.transport_pool);
        let hedge_delay = resilience.hedging_delay;
        let alternate_backend = req.upstream_pool.as_ref().map(|upstream_pool| {
            Self::resolve_alternate_backend(upstream_pool, pending_forward.backend_index)
        });
        let trace_span_for_upstream = req.trace_span.clone();
        let pending_forward_for_upstream = Arc::clone(&pending_forward);
        let (result_tx, result_rx) = oneshot::channel::<UpstreamResult>();
        let tunnel_mode = req.tunnel_mode;
        let bodyless_mode = req.bodyless_mode;
        let request_id = req.request_id;
        let method_idempotent = is_idempotent_method(&req.method);
        let hedge_method_allowed = resilience.hedging_method_allowed(&req.method);
        let hedge_configured = resilience.hedging_route_enabled_for(&route_name);
        let hedge_tunnel_request = req.tunnel_mode != TunnelMode::None;
        let policy = ForwardingRetryHedgePolicy::new(
            method_idempotent,
            bodyless_mode,
            hedge_method_allowed,
            hedge_configured,
            hedge_tunnel_request,
        );
        let fut = async move {
            let mut policy_telemetry = ForwardingPolicyTelemetry::default();
            let result: ForwardResult = async {
                retry_budget.mark_primary(&route_name);

                let forward_success: ForwardSuccess = if tunnel_mode == TunnelMode::Websocket
                    && backend_endpoint.scheme() == BackendScheme::Http
                {
                    let Some(body_rx) = websocket_tunnel_body_rx else {
                        return Err(ProxyError::Transport(
                            "websocket H1 tunnels require a downstream body channel".into(),
                        ));
                    };
                    Self::forward_http1_websocket_tunnel(
                        backend_endpoint.clone(),
                        Arc::clone(&pending_forward_for_upstream),
                        body_rx,
                        backend_timeout,
                        Arc::clone(&metrics),
                    )
                    .await?
                } else {
                    let request = request.ok_or_else(|| {
                        ProxyError::Transport(
                            "missing upstream request for non-websocket forward".into(),
                        )
                    })?;
                    let response: Response<Incoming> = match policy
                        .hedge_before_delay(alternate_backend.as_ref())
                    {
                        HedgePolicyDecision::WaitForPrimary => {
                            let hedge_candidate = Self::build_alternate_bodyless_candidate(
                                alternate_backend.as_ref(),
                                backend_endpoints.as_ref(),
                                pending_forward_for_upstream.as_ref(),
                            );

                            if let Some(AlternateBodylessCandidate {
                                backend: hedge_backend,
                                request: hedge_request,
                            }) = hedge_candidate
                            {
                            let primary_started = Instant::now();
                            let primary_backend = fwd_addr.clone();
                            let primary_fut = Self::send_upstream_request(
                                primary_backend,
                                request,
                                Arc::clone(&cb),
                                Arc::clone(&transport),
                            );
                            tokio::pin!(primary_fut);
                            let hedge_sleep = tokio::time::sleep(hedge_delay);
                            tokio::pin!(hedge_sleep);

                            if let Some(result) = tokio::select! {
                                result = &mut primary_fut => Some(result),
                                _ = &mut hedge_sleep => None,
                            } {
                                result?
                            } else {
                                match policy.hedge_after_delay(
                                    retry_budget_available_for_error(
                                        &ProxyError::Timeout,
                                        &route_name,
                                        retry_budget.as_ref(),
                                    ),
                                ) {
                                    HedgePolicyDecision::Hedge { reason } => {
                                        policy_telemetry.hedge.record_trigger(reason);
                                        let hedge_fut = Self::send_upstream_request(
                                            hedge_backend,
                                            hedge_request,
                                            Arc::clone(&cb),
                                            Arc::clone(&transport),
                                        );
                                        tokio::pin!(hedge_fut);
                                        tokio::select! {
                                            result = &mut primary_fut => {
                                                match result {
                                                    Ok(response) => {
                                                        policy_telemetry
                                                            .hedge
                                                            .record_outcome(HedgeOutcomeTelemetryReason::PrimaryWonAfterTrigger);
                                                        response
                                                    }
                                                    Err(_primary_err) => {
                                                        policy_telemetry
                                                            .hedge
                                                            .record_outcome(HedgeOutcomeTelemetryReason::HedgeWon);
                                                        let elapsed_ms = primary_started.elapsed().as_millis() as u64;
                                                        let delay_ms = hedge_delay.as_millis() as u64;
                                                        policy_telemetry
                                                            .hedge
                                                            .observe_primary_late_ms(elapsed_ms.saturating_sub(delay_ms));
                                                        hedge_fut.await?
                                                    }
                                                }
                                            },
                                            result = &mut hedge_fut => {
                                                match result {
                                                    Ok(response) => {
                                                        policy_telemetry
                                                            .hedge
                                                            .record_outcome(HedgeOutcomeTelemetryReason::HedgeWon);
                                                        let elapsed_ms = primary_started.elapsed().as_millis() as u64;
                                                        let delay_ms = hedge_delay.as_millis() as u64;
                                                        policy_telemetry
                                                            .hedge
                                                            .observe_primary_late_ms(elapsed_ms.saturating_sub(delay_ms));
                                                        response
                                                    }
                                                    Err(_hedge_err) => {
                                                        policy_telemetry
                                                            .hedge
                                                            .record_outcome(HedgeOutcomeTelemetryReason::PrimaryWonAfterTrigger);
                                                        primary_fut.await?
                                                    }
                                                }
                                            },
                                        }
                                    }
                                    HedgePolicyDecision::WaitForPrimary => primary_fut.await?,
                                    HedgePolicyDecision::DoNotHedge { denial } => {
                                        debug!(
                                            "hedge decision: request_id={} upstream={} decision=suppressed reason={} detail={:?}",
                                            request_id,
                                            route_name,
                                            HedgeDecisionReason::from(denial).slug(),
                                            denial
                                        );
                                        primary_fut.await?
                                    }
                                }
                            }
                            } else {
                                Self::send_upstream_request(
                                    fwd_addr.clone(),
                                    request,
                                    Arc::clone(&cb),
                                    Arc::clone(&transport),
                                )
                                .await?
                            }
                        }
                        HedgePolicyDecision::DoNotHedge { denial } => {
                            debug!(
                                "hedge decision: request_id={} upstream={} decision=denied reason={} detail={:?}",
                                request_id,
                                route_name,
                                HedgeDecisionReason::from(denial).slug(),
                                denial
                            );
                            match Self::send_upstream_request(
                                fwd_addr.clone(),
                                request,
                                Arc::clone(&cb),
                                Arc::clone(&transport),
                            )
                            .await
                            {
                                Ok(response) => response,
                                Err(primary_err) => Self::retry_primary_error(
                                        primary_err,
                                        RetryExecutionCtx {
                                            request_id,
                                            route_name: &route_name,
                                            policy,
                                            policy_telemetry: &mut policy_telemetry,
                                            retry_budget: retry_budget.as_ref(),
                                            alternate_backend: alternate_backend.as_ref(),
                                            backend_endpoints: backend_endpoints.as_ref(),
                                            pending_forward: pending_forward_for_upstream.as_ref(),
                                            circuit_breakers: Arc::clone(&cb),
                                            transport: Arc::clone(&transport),
                                        },
                                    )
                                    .await?,
                            }
                        }
                        HedgePolicyDecision::Hedge { reason } => {
                            debug!(
                                "hedge decision: request_id={} upstream={} decision=triggered reason={} detail={:?}",
                                request_id,
                                route_name,
                                HedgeDecisionReason::DelayElapsed.slug(),
                                reason
                            );
                            match Self::send_upstream_request(
                                fwd_addr.clone(),
                                request,
                                Arc::clone(&cb),
                                Arc::clone(&transport),
                            )
                            .await
                            {
                                Ok(response) => response,
                                Err(primary_err) => Self::retry_primary_error(
                                        primary_err,
                                        RetryExecutionCtx {
                                            request_id,
                                            route_name: &route_name,
                                            policy,
                                            policy_telemetry: &mut policy_telemetry,
                                            retry_budget: retry_budget.as_ref(),
                                            alternate_backend: alternate_backend.as_ref(),
                                            backend_endpoints: backend_endpoints.as_ref(),
                                            pending_forward: pending_forward_for_upstream.as_ref(),
                                            circuit_breakers: Arc::clone(&cb),
                                            transport: Arc::clone(&transport),
                                        },
                                    )
                                    .await?,
                            }
                        }
                    };

                    let (parts, body) = response.into_parts();
                    ForwardSuccess::Response {
                        status: parts.status,
                        headers: parts.headers,
                        body,
                    }
                };
                Ok(forward_success)
            }
            .await;
            let _ = result_tx.send(UpstreamResult {
                forward: result,
                policy: policy_telemetry,
            });
        };
        let spawned = match trace_span_for_upstream {
            Some(span) => spawn_async_task(fut.instrument(span), "upstream"),
            None => spawn_async_task(fut, "upstream"),
        };
        if !spawned {
            return Err(ProxyError::Transport(
                "dropping upstream task: no runtime available".into(),
            ));
        }
        Ok(result_rx)
    }
}
