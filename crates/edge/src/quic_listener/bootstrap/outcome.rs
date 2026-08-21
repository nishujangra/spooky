use std::time::Instant;

use http::StatusCode;
use impulse_errors::{ProxyError, classify_upstream_proxy_error};

use super::request::BootstrapPreparedRoute;
use crate::{
    Metrics, OverloadShedReason,
    runtime::connection::outcome::{
        AdmissionOutcomeClass, BackendOutcomeTarget, RouteOutcomeTarget, observe_admission_outcome,
        observe_backend_response_status_and_log, observe_proxy_error_outcome,
        observe_status_outcome,
    },
};

pub(in crate::quic_listener) fn bootstrap_route_target<'a>(
    route: &'a str,
) -> RouteOutcomeTarget<'a> {
    RouteOutcomeTarget { route }
}

pub(in crate::quic_listener) fn bootstrap_backend_target<'a>(
    upstream_name: &'a str,
    backend_addr: &'a str,
    backend_index: usize,
) -> BackendOutcomeTarget<'a> {
    BackendOutcomeTarget {
        upstream: upstream_name,
        backend_addr: Some(backend_addr),
        backend_index: Some(backend_index),
    }
}

pub(in crate::quic_listener) fn bootstrap_route_target_for_prepared(
    prepared_route: &BootstrapPreparedRoute,
) -> RouteOutcomeTarget<'_> {
    bootstrap_route_target(&prepared_route.upstream_name)
}

pub(in crate::quic_listener) fn bootstrap_backend_target_for_prepared(
    prepared_route: &BootstrapPreparedRoute,
) -> BackendOutcomeTarget<'_> {
    bootstrap_backend_target(
        &prepared_route.upstream_name,
        &prepared_route.backend_addr,
        prepared_route.backend_index,
    )
}

pub(in crate::quic_listener) fn observe_bootstrap_admission_outcome(
    metrics: &Metrics,
    upstream_name: &str,
    backend_addr: &str,
    backend_index: usize,
    request_start: Instant,
    status: StatusCode,
    outcome: AdmissionOutcomeClass,
) {
    let _ = observe_admission_outcome(
        metrics,
        bootstrap_route_target(upstream_name),
        Some(bootstrap_backend_target(
            upstream_name,
            backend_addr,
            backend_index,
        )),
        request_start.elapsed(),
        status,
        outcome,
    );
}

pub(in crate::quic_listener) fn observe_bootstrap_request_proxy_error(
    metrics: &Metrics,
    upstream_name: &str,
    backend_addr: &str,
    backend_index: usize,
    request_start: Instant,
    status: StatusCode,
    proxy_err: &ProxyError,
) {
    let _ = observe_proxy_error_outcome(
        metrics,
        bootstrap_route_target(upstream_name),
        Some(bootstrap_backend_target(
            upstream_name,
            backend_addr,
            backend_index,
        )),
        request_start.elapsed(),
        Some(status),
        proxy_err,
        None,
    );
}

pub(in crate::quic_listener) fn observe_bootstrap_dispatch_failure(
    prepared_route: &BootstrapPreparedRoute,
    metrics: &Metrics,
    request_start: Instant,
    request_id: u64,
    status: StatusCode,
    proxy_err: &ProxyError,
) {
    let _ = observe_proxy_error_outcome(
        metrics,
        bootstrap_route_target_for_prepared(prepared_route),
        Some(bootstrap_backend_target_for_prepared(prepared_route)),
        request_start.elapsed(),
        Some(status),
        proxy_err,
        None,
    );
    if let Some(classified) = classify_upstream_proxy_error(proxy_err) {
        crate::quic_listener::QUICListener::log_classified_upstream_failure(
            "bootstrap",
            Some(request_id),
            Some(&prepared_route.upstream_name),
            &prepared_route.backend_addr,
            &classified,
        );
        let _ = crate::runtime::connection::outcome::observe_classified_backend_failure_and_log(
            crate::runtime::connection::outcome::ClassifiedBackendFailureInput {
                metrics_phase: "bootstrap",
                upstream_name: &prepared_route.upstream_name,
                backend_addr: &prepared_route.backend_addr,
                backend_index: prepared_route.backend_index,
                upstream_pool: Some(&prepared_route.upstream_pool),
                metrics,
                classified: &classified,
            },
        );
    } else {
        log::warn!(
            "upstream failure: upstream={} backend={} failure_class=unclassified detail={}",
            prepared_route.upstream_name,
            prepared_route.backend_addr,
            proxy_err
        );
    }
}

pub(in crate::quic_listener) fn observe_bootstrap_response_status(
    metrics: &Metrics,
    prepared_route: &BootstrapPreparedRoute,
    request_start: Instant,
    status: StatusCode,
) {
    let _ = observe_status_outcome(
        metrics,
        bootstrap_route_target_for_prepared(prepared_route),
        Some(bootstrap_backend_target_for_prepared(prepared_route)),
        request_start.elapsed(),
        status,
    );
    let _ = observe_backend_response_status_and_log(
        crate::runtime::connection::outcome::BackendHealthObservationInput {
            backend_addr: &prepared_route.backend_addr,
            backend_index: prepared_route.backend_index,
            upstream_pool: Some(&prepared_route.upstream_pool),
            status,
        },
    );
}

pub(in crate::quic_listener) fn observe_bootstrap_response_prebuffer_overflow(
    metrics: &Metrics,
    prepared_route: &BootstrapPreparedRoute,
    request_start: Instant,
) {
    let _ = observe_proxy_error_outcome(
        metrics,
        bootstrap_route_target_for_prepared(prepared_route),
        Some(bootstrap_backend_target_for_prepared(prepared_route)),
        request_start.elapsed(),
        Some(StatusCode::SERVICE_UNAVAILABLE),
        &ProxyError::Pool(impulse_errors::PoolError::BackendOverloaded(
            "response prebuffer cap".into(),
        )),
        Some(OverloadShedReason::ResponsePrebufferCap),
    );
}

pub(in crate::quic_listener) fn finish_bootstrap_backend_request_accounting(
    prepared_route: &BootstrapPreparedRoute,
    request_start: Instant,
    status: Option<u16>,
) {
    crate::runtime::connection::outcome::finish_backend_request_accounting(
        crate::runtime::connection::outcome::BackendRequestFinishInput {
            upstream_pool: Some(&prepared_route.upstream_pool),
            backend_index: Some(prepared_route.backend_index),
            elapsed: request_start.elapsed(),
            status,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        observability::RequestOutcomeReason,
        quic_listener::bootstrap::request::{
            BootstrapRejectionReason, BootstrapTerminalOutcome, BootstrapTimeoutReason,
        },
        runtime::connection::outcome::{
            BackendOutcomeTarget, RouteOutcomeTarget, observe_admission_outcome,
            observe_proxy_error_outcome,
        },
    };

    fn upstream_request_count(
        metrics: &Metrics,
        upstream: &str,
        status_class: &str,
        outcome: &str,
    ) -> u64 {
        metrics
            .snapshot_upstream_request_counts()
            .into_iter()
            .find(|(key, _)| {
                key.upstream == upstream
                    && key.status_class == status_class
                    && key.outcome == outcome
            })
            .map(|(_, count)| count)
            .unwrap_or_default()
    }

    fn backend_request_count(
        metrics: &Metrics,
        upstream: &str,
        backend: &str,
        status_class: &str,
        outcome: &str,
    ) -> u64 {
        metrics
            .snapshot_backend_request_counts()
            .into_iter()
            .find(|(key, _)| {
                key.upstream == upstream
                    && key.backend == backend
                    && key.status_class == status_class
                    && key.outcome == outcome
            })
            .map(|(_, count)| count)
            .unwrap_or_default()
    }

    #[test]
    fn bootstrap_and_quic_timeout_failures_record_same_reason_and_label_set() {
        let bootstrap_metrics = Metrics::new(1, [String::from("api")]);
        let forwarding_metrics = Metrics::new(1, [String::from("api")]);
        let request_start = Instant::now() - std::time::Duration::from_millis(25);

        observe_bootstrap_request_proxy_error(
            &bootstrap_metrics,
            "api",
            "backend-a",
            0,
            request_start,
            StatusCode::GATEWAY_TIMEOUT,
            &ProxyError::Timeout,
        );
        let forwarding = observe_proxy_error_outcome(
            &forwarding_metrics,
            RouteOutcomeTarget { route: "api" },
            Some(BackendOutcomeTarget {
                upstream: "api",
                backend_addr: Some("backend-a"),
                backend_index: Some(0),
            }),
            request_start.elapsed(),
            Some(StatusCode::GATEWAY_TIMEOUT),
            &ProxyError::Timeout,
            None,
        );

        assert_eq!(
            BootstrapTerminalOutcome::TimedOut(BootstrapTimeoutReason::Upstream).canonical_reason(),
            Some(RequestOutcomeReason::TimedOut)
        );
        assert_eq!(
            forwarding.route_outcome,
            crate::runtime::connection::outcome::CanonicalRouteOutcome::Timeout
        );
        assert_eq!(
            upstream_request_count(&bootstrap_metrics, "api", "5xx", "timeout"),
            1
        );
        assert_eq!(
            upstream_request_count(&forwarding_metrics, "api", "5xx", "timeout"),
            1
        );
        assert_eq!(
            backend_request_count(&bootstrap_metrics, "api", "backend-a", "5xx", "timeout"),
            1
        );
        assert_eq!(
            backend_request_count(&forwarding_metrics, "api", "backend-a", "5xx", "timeout"),
            1
        );
    }

    #[test]
    fn bootstrap_and_quic_admission_failures_record_same_reason_and_label_set() {
        let bootstrap_metrics = Metrics::new(1, [String::from("api")]);
        let forwarding_metrics = Metrics::new(1, [String::from("api")]);
        let request_start = Instant::now() - std::time::Duration::from_millis(5);

        observe_bootstrap_admission_outcome(
            &bootstrap_metrics,
            "api",
            "backend-a",
            0,
            request_start,
            StatusCode::SERVICE_UNAVAILABLE,
            AdmissionOutcomeClass::OverloadShed {
                reason: Some(OverloadShedReason::GlobalInflight),
            },
        );
        let forwarding = observe_admission_outcome(
            &forwarding_metrics,
            RouteOutcomeTarget { route: "api" },
            Some(BackendOutcomeTarget {
                upstream: "api",
                backend_addr: Some("backend-a"),
                backend_index: Some(0),
            }),
            request_start.elapsed(),
            StatusCode::SERVICE_UNAVAILABLE,
            AdmissionOutcomeClass::OverloadShed {
                reason: Some(OverloadShedReason::GlobalInflight),
            },
        );

        assert_eq!(
            BootstrapTerminalOutcome::Rejected(BootstrapRejectionReason::Overloaded)
                .canonical_reason(),
            Some(RequestOutcomeReason::Overloaded)
        );
        assert_eq!(
            forwarding.route_outcome,
            crate::runtime::connection::outcome::CanonicalRouteOutcome::OverloadShed
        );
        assert_eq!(
            upstream_request_count(&bootstrap_metrics, "api", "5xx", "overload_shed"),
            1
        );
        assert_eq!(
            upstream_request_count(&forwarding_metrics, "api", "5xx", "overload_shed"),
            1
        );
        assert_eq!(
            backend_request_count(
                &bootstrap_metrics,
                "api",
                "backend-a",
                "5xx",
                "overload_shed"
            ),
            1
        );
        assert_eq!(
            backend_request_count(
                &forwarding_metrics,
                "api",
                "backend-a",
                "5xx",
                "overload_shed"
            ),
            1
        );
    }
}
