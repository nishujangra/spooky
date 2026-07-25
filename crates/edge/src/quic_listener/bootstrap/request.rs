use std::{
    convert::Infallible,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};

use bytes::Bytes;
use http::{HeaderMap, Request, Response, StatusCode};
use http_body_util::{BodyExt, combinators::BoxBody};
use hyper::body::Incoming;
use log::warn;
use spooky_bridge::{
    BridgeError,
    request::{
        RequestBuildInput, RequestBuildPolicies, RequestBuildTarget, RequestForwardedContext,
        RequestTraceContext, build_h1_request, build_h2_request_for_target,
    },
};
use spooky_config::{
    backend_endpoint::{BackendEndpoint, BackendScheme},
    runtime::RuntimeUpstreamPolicy,
};
use spooky_errors::ProxyError;
use spooky_lb::upstream_pool::UpstreamPool;

use super::{
    super::{
        QUICListener,
        admission::{
            AdmissionPolicyDecision, admission_rejection_response,
            evaluate_forwarding_pre_admission_policy,
        },
        forwarding::BootstrapResolutionInput,
    },
    context::BootstrapRequestCtx,
    intake::{BootstrapRequestIntake, bootstrap_error_response},
    outcome::{observe_bootstrap_admission_outcome, observe_bootstrap_request_proxy_error},
    response::{BootstrapStreamingBody, boxed_full},
};
use crate::runtime::connection::outcome::AdmissionOutcomeClass;

/// The bootstrap request lifecycle stages, mirroring the QUIC data path so the
/// same reasoning applies. Some stages (`Intake`, `WriteResponse`, `Terminalize`)
/// name the full model for completeness but do not currently emit a distinct
/// terminal *rejection* — intake failures return earlier and successful writeback
/// is not an error terminal — so they are not yet constructed.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::quic_listener) enum BootstrapLifecycleStage {
    Intake,
    Validate,
    ResolveRoute,
    AdmitOrReject,
    Dispatch,
    WriteResponse,
    Terminalize,
}

impl BootstrapLifecycleStage {
    pub(in crate::quic_listener) fn slug(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Validate => "validate",
            Self::ResolveRoute => "resolve_route",
            Self::AdmitOrReject => "admit_or_reject",
            Self::Dispatch => "dispatch",
            Self::WriteResponse => "write_response",
            Self::Terminalize => "terminalize",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::quic_listener) enum BootstrapRequestMode {
    Standard,
    WebsocketUpgrade,
}

impl BootstrapRequestMode {
    pub(in crate::quic_listener) fn is_websocket_upgrade(self) -> bool {
        matches!(self, Self::WebsocketUpgrade)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::quic_listener) enum BootstrapRejectionReason {
    ValidationFailed,
    AuthDenied,
    RateLimited,
    Overloaded,
    RequestBodyTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::quic_listener) enum BootstrapBackendFailureReason {
    RouteResolutionFailed,
    MissingEndpoint,
    RequestBuildFailed,
    DispatchFailed,
}

/// Bootstrap timeout classification. `ResponseBody` names the model's
/// response-body-phase timeout for parity with the QUIC data path; bootstrap does
/// not yet detect that phase distinctly, so only `Upstream` is currently emitted.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::quic_listener) enum BootstrapTimeoutReason {
    Upstream,
    ResponseBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::quic_listener) enum BootstrapTerminalOutcome {
    AcceptedStandardResponse,
    AcceptedWebsocketUpgrade,
    Rejected(BootstrapRejectionReason),
    BackendFailed(BootstrapBackendFailureReason),
    TimedOut(BootstrapTimeoutReason),
}

impl BootstrapTerminalOutcome {
    /// Whether this terminal outcome represents an accepted (non-error) response,
    /// mirroring the QUIC data path's success terminals.
    pub(in crate::quic_listener) fn is_accepted(self) -> bool {
        matches!(
            self,
            Self::AcceptedStandardResponse | Self::AcceptedWebsocketUpgrade
        )
    }

    /// A stable slug naming the terminal classification, aligned with the
    /// terminal-state names used in the QUIC data path.
    pub(in crate::quic_listener) fn slug(self) -> &'static str {
        match self {
            Self::AcceptedStandardResponse => "accepted_standard_response",
            Self::AcceptedWebsocketUpgrade => "accepted_websocket_upgrade",
            Self::Rejected(reason) => match reason {
                BootstrapRejectionReason::ValidationFailed => "rejected_validation",
                BootstrapRejectionReason::AuthDenied => "rejected_auth_denied",
                BootstrapRejectionReason::RateLimited => "rejected_rate_limited",
                BootstrapRejectionReason::Overloaded => "rejected_overloaded",
                BootstrapRejectionReason::RequestBodyTooLarge => "rejected_request_body_too_large",
            },
            Self::BackendFailed(reason) => match reason {
                BootstrapBackendFailureReason::RouteResolutionFailed => {
                    "backend_failed_route_resolution"
                }
                BootstrapBackendFailureReason::MissingEndpoint => "backend_failed_missing_endpoint",
                BootstrapBackendFailureReason::RequestBuildFailed => "backend_failed_request_build",
                BootstrapBackendFailureReason::DispatchFailed => "backend_failed_dispatch",
            },
            Self::TimedOut(reason) => match reason {
                BootstrapTimeoutReason::Upstream => "timed_out_upstream",
                BootstrapTimeoutReason::ResponseBody => "timed_out_response_body",
            },
        }
    }
}

pub(in crate::quic_listener) struct BootstrapTerminalResponse {
    pub(in crate::quic_listener) stage: BootstrapLifecycleStage,
    pub(in crate::quic_listener) outcome: BootstrapTerminalOutcome,
    pub(in crate::quic_listener) response: Response<BoxBody<Bytes, Infallible>>,
}

impl BootstrapTerminalResponse {
    pub(in crate::quic_listener) fn new(
        stage: BootstrapLifecycleStage,
        outcome: BootstrapTerminalOutcome,
        response: Response<BoxBody<Bytes, Infallible>>,
    ) -> Box<Self> {
        Box::new(Self {
            stage,
            outcome,
            response,
        })
    }

    pub(in crate::quic_listener) fn into_response(self) -> Response<BoxBody<Bytes, Infallible>> {
        self.response
    }

    /// Log the terminal state at the terminalization boundary, then return the
    /// response. Phase 11: the terminal outcome is now *observed* rather than a
    /// dead byproduct, so bootstrap terminalization is traceable the same way the
    /// QUIC data path's terminal states are.
    pub(in crate::quic_listener) fn into_observed_response(
        self,
        request_id: u64,
    ) -> Response<BoxBody<Bytes, Infallible>> {
        if !self.outcome.is_accepted() {
            log::debug!(
                "bootstrap request {} terminalized at stage={} outcome={}",
                request_id,
                self.stage.slug(),
                self.outcome.slug()
            );
        }
        self.response
    }
}

pub(in crate::quic_listener) type BootstrapTerminalResult<T> =
    Result<T, Box<BootstrapTerminalResponse>>;

pub(in crate::quic_listener) struct BootstrapPreparedRoute {
    pub(in crate::quic_listener) endpoint: BackendEndpoint,
    pub(in crate::quic_listener) backend_addr: String,
    pub(in crate::quic_listener) backend_index: usize,
    pub(in crate::quic_listener) upstream_name: String,
    pub(in crate::quic_listener) upstream_policy: RuntimeUpstreamPolicy,
    pub(in crate::quic_listener) upstream_pool: Arc<RwLock<UpstreamPool>>,
}

pub(in crate::quic_listener) struct BootstrapPolicyEvaluationInput<'a> {
    pub(in crate::quic_listener) intake: &'a BootstrapRequestIntake,
    pub(in crate::quic_listener) headers: &'a HeaderMap,
    pub(in crate::quic_listener) request_ctx: BootstrapRequestCtx<'a>,
}

pub(in crate::quic_listener) struct BootstrapBuildRequestInput<'a> {
    pub(in crate::quic_listener) request: Request<Incoming>,
    pub(in crate::quic_listener) intake: &'a BootstrapRequestIntake,
    pub(in crate::quic_listener) prepared_route: &'a BootstrapPreparedRoute,
    pub(in crate::quic_listener) request_ctx: BootstrapRequestCtx<'a>,
    pub(in crate::quic_listener) request_id: u64,
    pub(in crate::quic_listener) traceparent: Option<&'a str>,
}

fn bootstrap_bridge_headers(headers: &HeaderMap) -> Vec<quiche::h3::Header> {
    headers
        .iter()
        .map(|(name, value)| quiche::h3::Header::new(name.as_str().as_bytes(), value.as_bytes()))
        .collect()
}

fn bootstrap_request_build_target<'a>(
    endpoint: &'a BackendEndpoint,
    upstream_policy: &'a RuntimeUpstreamPolicy,
) -> RequestBuildTarget<'a> {
    RequestBuildTarget {
        endpoint,
        policies: RequestBuildPolicies {
            host_policy: &upstream_policy.host.0,
            forwarded_header_policy: &upstream_policy.forwarded_headers.0,
        },
    }
}

fn bootstrap_request_build_input<'a>(
    intake: &'a BootstrapRequestIntake,
    headers: &'a [quiche::h3::Header],
    body: BoxBody<Bytes, Infallible>,
    content_length: Option<usize>,
    request_ctx: BootstrapRequestCtx<'a>,
    request_id: u64,
    traceparent: Option<&'a str>,
) -> RequestBuildInput<'a, BoxBody<Bytes, Infallible>> {
    RequestBuildInput {
        method: &intake.method,
        path: &intake.path,
        authority: intake.authority.as_deref(),
        headers,
        body,
        content_length,
        body_mode: RequestBuildInput::<BoxBody<Bytes, Infallible>>::body_mode_for_length(
            content_length,
        ),
        trace: RequestTraceContext {
            request_id,
            traceparent,
        },
        forwarded: RequestForwardedContext {
            client_addr: request_ctx.peer,
        },
    }
}

fn internal_proxy_error_response(alt_svc: &str) -> Response<BoxBody<Bytes, Infallible>> {
    bootstrap_error_response(
        alt_svc,
        StatusCode::INTERNAL_SERVER_ERROR,
        b"internal proxy error\n",
    )
}

fn resolve_scoped_rate_limit_key_for_bootstrap(
    rule: &crate::resilience::scoped_rate_limit::ScopedRateLimitRule,
    upstream_name: &str,
    intake: &BootstrapRequestIntake,
    peer: SocketAddr,
    lb_header_lookup: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    QUICListener::resolve_scoped_rate_limit_key(
        rule,
        upstream_name,
        &intake.method,
        &intake.path,
        intake.authority.as_deref(),
        peer,
        Some(lb_header_lookup),
    )
}

pub(in crate::quic_listener) fn evaluate_bootstrap_request_policy(
    input: BootstrapPolicyEvaluationInput<'_>,
) -> BootstrapTerminalResult<BootstrapPreparedRoute> {
    let lb_header_lookup = |name: &str| {
        input
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };

    let resolved = match QUICListener::resolve_bootstrap_target(BootstrapResolutionInput {
        method: &input.intake.method,
        path: &input.intake.path,
        authority: input.intake.authority.as_deref(),
        header_lookup: Some(&lb_header_lookup),
        routing_index: &input.request_ctx.runtime.routing_index,
        upstream_pools: &input.request_ctx.runtime.upstream_pools,
        upstream_policies: &input.request_ctx.runtime.upstream_policies,
        metrics: input.request_ctx.runtime.metrics.as_ref(),
        elapsed: Duration::ZERO,
    }) {
        Ok(value) => value,
        Err(err) => {
            let (status, body) = QUICListener::bootstrap_route_resolution_error_response(&err);
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::ResolveRoute,
                BootstrapTerminalOutcome::BackendFailed(
                    BootstrapBackendFailureReason::RouteResolutionFailed,
                ),
                bootstrap_error_response(&input.request_ctx.runtime.alt_svc, status, body),
            ));
        }
    };

    let admission = evaluate_forwarding_pre_admission_policy(
        &resolved.upstream_policy,
        Some(&lb_header_lookup),
        &input.request_ctx.runtime.resilience.brownout,
        input
            .request_ctx
            .runtime
            .resilience
            .adaptive_admission
            .inflight_percent(),
        &resolved.upstream_name,
        input
            .request_ctx
            .runtime
            .resilience
            .shed_retry_after_seconds,
        &input.request_ctx.runtime.resilience.scoped_rate_limits,
        |rule| {
            resolve_scoped_rate_limit_key_for_bootstrap(
                rule,
                &resolved.upstream_name,
                input.intake,
                input.request_ctx.peer,
                &lb_header_lookup,
            )
        },
    );
    input
        .request_ctx
        .runtime
        .metrics
        .set_brownout_active(input.request_ctx.runtime.resilience.brownout.is_active());
    let rejection_response = admission_rejection_response(&admission);

    match admission {
        AdmissionPolicyDecision::AdmitReady => {}
        AdmissionPolicyDecision::Unauthorized(_) => {
            input.request_ctx.runtime.metrics.inc_policy_denied();
            observe_bootstrap_admission_outcome(
                input.request_ctx.runtime.metrics.as_ref(),
                &resolved.upstream_name,
                &resolved.backend_addr,
                resolved.backend_index,
                input.request_ctx.request_start,
                StatusCode::UNAUTHORIZED,
                AdmissionOutcomeClass::AuthDenied,
            );
            warn!(
                "Bootstrap request route={} denied by auth policy",
                resolved.upstream_name
            );
            let Some(response) = rejection_response.as_ref() else {
                warn!(
                    "Bootstrap request route={} missing admission rejection response for unauthorized decision",
                    resolved.upstream_name
                );
                return Err(BootstrapTerminalResponse::new(
                    BootstrapLifecycleStage::AdmitOrReject,
                    BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::AuthDenied),
                    internal_proxy_error_response(&input.request_ctx.runtime.alt_svc),
                ));
            };
            let Some(challenge) = response.www_authenticate else {
                warn!(
                    "Bootstrap request route={} missing auth challenge in admission rejection response",
                    resolved.upstream_name
                );
                return Err(BootstrapTerminalResponse::new(
                    BootstrapLifecycleStage::AdmitOrReject,
                    BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::AuthDenied),
                    internal_proxy_error_response(&input.request_ctx.runtime.alt_svc),
                ));
            };
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::AdmitOrReject,
                BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::AuthDenied),
                Response::builder()
                    .status(response.status)
                    .header("alt-svc", &input.request_ctx.runtime.alt_svc)
                    .header("www-authenticate", challenge)
                    .body(boxed_full(Bytes::from_static(response.body)))
                    .unwrap_or_else(|_| Response::new(boxed_full(Bytes::from_static(b"error\n")))),
            ));
        }
        AdmissionPolicyDecision::RateLimited(decision) => {
            input.request_ctx.runtime.metrics.inc_request_rate_limited();
            observe_bootstrap_admission_outcome(
                input.request_ctx.runtime.metrics.as_ref(),
                &resolved.upstream_name,
                &resolved.backend_addr,
                resolved.backend_index,
                input.request_ctx.request_start,
                StatusCode::TOO_MANY_REQUESTS,
                AdmissionOutcomeClass::RateLimited,
            );
            warn!(
                "Bootstrap request route={} scoped rate limit exceeded by rule={}",
                decision.route, decision.rule_name
            );
            let Some(response) = rejection_response.as_ref() else {
                warn!(
                    "Bootstrap request route={} missing admission rejection response for rate-limited decision",
                    resolved.upstream_name
                );
                return Err(BootstrapTerminalResponse::new(
                    BootstrapLifecycleStage::AdmitOrReject,
                    BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::RateLimited),
                    internal_proxy_error_response(&input.request_ctx.runtime.alt_svc),
                ));
            };
            let Some(retry_after_seconds) = response.retry_after_seconds else {
                warn!(
                    "Bootstrap request route={} missing retry-after in rate-limited admission rejection response",
                    resolved.upstream_name
                );
                return Err(BootstrapTerminalResponse::new(
                    BootstrapLifecycleStage::AdmitOrReject,
                    BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::RateLimited),
                    internal_proxy_error_response(&input.request_ctx.runtime.alt_svc),
                ));
            };
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::AdmitOrReject,
                BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::RateLimited),
                Response::builder()
                    .status(response.status)
                    .header("alt-svc", &input.request_ctx.runtime.alt_svc)
                    .header("retry-after", retry_after_seconds.to_string())
                    .body(boxed_full(Bytes::from_static(response.body)))
                    .unwrap_or_else(|_| Response::new(boxed_full(Bytes::from_static(b"error\n")))),
            ));
        }
        AdmissionPolicyDecision::Overloaded(decision) => {
            observe_bootstrap_admission_outcome(
                input.request_ctx.runtime.metrics.as_ref(),
                &resolved.upstream_name,
                &resolved.backend_addr,
                resolved.backend_index,
                input.request_ctx.request_start,
                StatusCode::SERVICE_UNAVAILABLE,
                AdmissionOutcomeClass::OverloadShed {
                    reason: Some(decision.reason.metrics_reason()),
                },
            );
            input
                .request_ctx
                .runtime
                .resilience
                .adaptive_admission
                .observe(input.request_ctx.request_start.elapsed(), true);
            let Some(response) = rejection_response.as_ref() else {
                warn!(
                    "Bootstrap request route={} missing admission rejection response for overload decision",
                    resolved.upstream_name
                );
                return Err(BootstrapTerminalResponse::new(
                    BootstrapLifecycleStage::AdmitOrReject,
                    BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::Overloaded),
                    internal_proxy_error_response(&input.request_ctx.runtime.alt_svc),
                ));
            };
            let Some(retry_after_seconds) = response.retry_after_seconds else {
                warn!(
                    "Bootstrap request route={} missing retry-after in overload admission rejection response",
                    resolved.upstream_name
                );
                return Err(BootstrapTerminalResponse::new(
                    BootstrapLifecycleStage::AdmitOrReject,
                    BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::Overloaded),
                    internal_proxy_error_response(&input.request_ctx.runtime.alt_svc),
                ));
            };
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::AdmitOrReject,
                BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::Overloaded),
                Response::builder()
                    .status(response.status)
                    .header("alt-svc", &input.request_ctx.runtime.alt_svc)
                    .header("retry-after", retry_after_seconds.to_string())
                    .body(boxed_full(Bytes::from_static(response.body)))
                    .unwrap_or_else(|_| Response::new(boxed_full(Bytes::from_static(b"error\n")))),
            ));
        }
    }

    let endpoint = match input
        .request_ctx
        .runtime
        .backend_endpoints
        .get(&resolved.backend_addr)
    {
        Some(endpoint) => endpoint.clone(),
        None => {
            observe_bootstrap_request_proxy_error(
                input.request_ctx.runtime.metrics.as_ref(),
                &resolved.upstream_name,
                &resolved.backend_addr,
                resolved.backend_index,
                input.request_ctx.request_start,
                StatusCode::BAD_GATEWAY,
                &ProxyError::Transport("no endpoint".into()),
            );
            return Err(BootstrapTerminalResponse::new(
                BootstrapLifecycleStage::ResolveRoute,
                BootstrapTerminalOutcome::BackendFailed(
                    BootstrapBackendFailureReason::MissingEndpoint,
                ),
                bootstrap_error_response(
                    &input.request_ctx.runtime.alt_svc,
                    StatusCode::BAD_GATEWAY,
                    b"no endpoint\n",
                ),
            ));
        }
    };

    Ok(BootstrapPreparedRoute {
        endpoint,
        backend_addr: resolved.backend_addr,
        backend_index: resolved.backend_index,
        upstream_name: resolved.upstream_name,
        upstream_policy: resolved.upstream_policy,
        upstream_pool: resolved.upstream_pool,
    })
}

pub(in crate::quic_listener) fn build_bootstrap_upstream_request(
    input: BootstrapBuildRequestInput<'_>,
) -> Result<Request<BoxBody<Bytes, Infallible>>, BridgeError> {
    let bridge_headers = bootstrap_bridge_headers(input.request.headers());
    let request_target = bootstrap_request_build_target(
        &input.prepared_route.endpoint,
        &input.prepared_route.upstream_policy,
    );

    if input.intake.request_mode.is_websocket_upgrade() {
        return build_h1_request(
            request_target,
            bootstrap_request_build_input(
                input.intake,
                &bridge_headers,
                boxed_full(Bytes::new()),
                None,
                input.request_ctx,
                input.request_id,
                input.traceparent,
            ),
        );
    }

    let bridge_body = BootstrapStreamingBody::new(input.request.into_body())
        .map_err(|never| match never {})
        .boxed();
    if input.prepared_route.endpoint.scheme() == BackendScheme::Http {
        build_h1_request(
            request_target,
            bootstrap_request_build_input(
                input.intake,
                &bridge_headers,
                bridge_body,
                None,
                input.request_ctx,
                input.request_id,
                input.traceparent,
            ),
        )
    } else {
        build_h2_request_for_target(
            request_target,
            bootstrap_request_build_input(
                input.intake,
                &bridge_headers,
                bridge_body,
                None,
                input.request_ctx,
                input.request_id,
                input.traceparent,
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_outcome_accepted_classification() {
        assert!(BootstrapTerminalOutcome::AcceptedStandardResponse.is_accepted());
        assert!(BootstrapTerminalOutcome::AcceptedWebsocketUpgrade.is_accepted());
        assert!(
            !BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::AuthDenied).is_accepted()
        );
        assert!(
            !BootstrapTerminalOutcome::BackendFailed(BootstrapBackendFailureReason::DispatchFailed)
                .is_accepted()
        );
        assert!(
            !BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream).is_accepted()
        );
    }

    #[test]
    fn terminal_outcome_slugs_are_distinct_and_stable() {
        let outcomes = [
            BootstrapTerminalOutcome::AcceptedStandardResponse,
            BootstrapTerminalOutcome::AcceptedWebsocketUpgrade,
            BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::ValidationFailed),
            BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::AuthDenied),
            BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::RateLimited),
            BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::Overloaded),
            BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::RequestBodyTooLarge),
            BootstrapTerminalOutcome::BackendFailed(
                BootstrapBackendFailureReason::RouteResolutionFailed,
            ),
            BootstrapTerminalOutcome::BackendFailed(BootstrapBackendFailureReason::MissingEndpoint),
            BootstrapTerminalOutcome::BackendFailed(
                BootstrapBackendFailureReason::RequestBuildFailed,
            ),
            BootstrapTerminalOutcome::BackendFailed(BootstrapBackendFailureReason::DispatchFailed),
            BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream),
            BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::ResponseBody),
        ];
        let mut slugs: Vec<&str> = outcomes.iter().map(|o| o.slug()).collect();
        let count = slugs.len();
        slugs.sort_unstable();
        slugs.dedup();
        assert_eq!(slugs.len(), count, "terminal outcome slugs must be unique");
        // Accepted outcomes carry accepted-prefixed slugs; everything else names its failure class.
        assert_eq!(
            BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream).slug(),
            "timed_out_upstream"
        );
    }

    #[test]
    fn websocket_upgrade_is_a_request_mode_state() {
        assert!(BootstrapRequestMode::WebsocketUpgrade.is_websocket_upgrade());
        assert!(!BootstrapRequestMode::Standard.is_websocket_upgrade());
    }
}
