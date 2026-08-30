use impulse_config::runtime::RuntimeUpstreamPolicy;

use super::{lb_key::ResolvedLbKey, *};
use crate::runtime::connection::outcome::{RouteOutcomeTarget, observe_proxy_error_outcome};

#[derive(Clone, Copy)]
pub(in crate::quic_listener) struct TargetResolutionRequest<'a> {
    pub(in crate::quic_listener) method: &'a str,
    pub(in crate::quic_listener) path: &'a str,
    pub(in crate::quic_listener) authority: Option<&'a str>,
    pub(in crate::quic_listener) cid_key: Option<&'a str>,
    pub(in crate::quic_listener) header_lookup: Option<&'a LbHeaderLookup<'a>>,
}

impl<'a> TargetResolutionRequest<'a> {
    pub(in crate::quic_listener) fn new(
        method: &'a str,
        path: &'a str,
        authority: Option<&'a str>,
        cid_key: Option<&'a str>,
        header_lookup: Option<&'a LbHeaderLookup<'a>>,
    ) -> Self {
        Self {
            method,
            path,
            authority,
            cid_key,
            header_lookup,
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::quic_listener) struct ResolutionContext<'a> {
    pub(in crate::quic_listener) routing_index: &'a RouteIndex,
    pub(in crate::quic_listener) upstream_pools: &'a HashMap<String, Arc<RwLock<UpstreamPool>>>,
    pub(in crate::quic_listener) upstream_policies: &'a HashMap<String, RuntimeUpstreamPolicy>,
}

impl<'a> ResolutionContext<'a> {
    pub(in crate::quic_listener) fn new(
        routing_index: &'a RouteIndex,
        upstream_pools: &'a HashMap<String, Arc<RwLock<UpstreamPool>>>,
        upstream_policies: &'a HashMap<String, RuntimeUpstreamPolicy>,
    ) -> Self {
        Self {
            routing_index,
            upstream_pools,
            upstream_policies,
        }
    }
}

#[derive(Clone, Copy)]
pub(in crate::quic_listener) struct ResolutionObservation<'a> {
    pub(in crate::quic_listener) metrics: &'a Metrics,
    pub(in crate::quic_listener) elapsed: Duration,
}

impl<'a> ResolutionObservation<'a> {
    pub(in crate::quic_listener) fn new(metrics: &'a Metrics, elapsed: Duration) -> Self {
        Self { metrics, elapsed }
    }
}

pub(in crate::quic_listener) struct RouteResolution {
    pub(in crate::quic_listener) upstream_name: String,
    pub(in crate::quic_listener) upstream_pool: Arc<RwLock<UpstreamPool>>,
    pub(in crate::quic_listener) upstream_policy: RuntimeUpstreamPolicy,
    pub(in crate::quic_listener) route_path_len: usize,
    pub(in crate::quic_listener) route_host_specific: bool,
    pub(in crate::quic_listener) route_reason: RouteDecisionReason,
}

pub(in crate::quic_listener) struct BackendSelection {
    pub(in crate::quic_listener) backend_addr: String,
    pub(in crate::quic_listener) backend_index: usize,
    pub(in crate::quic_listener) backend_lb: String,
}

pub(in crate::quic_listener) struct TargetResolution {
    pub(in crate::quic_listener) route: RouteResolution,
    pub(in crate::quic_listener) backend: BackendSelection,
}

pub(super) struct ForwardTargetResolution {
    pub(super) upstream_name: String,
    pub(super) upstream_pool: Arc<RwLock<UpstreamPool>>,
    pub(super) upstream_policy: RuntimeUpstreamPolicy,
    pub(super) route_path_len: usize,
    pub(super) route_host_specific: bool,
    pub(super) route_reason: String,
    pub(super) backend_addr: String,
    pub(super) backend_index: usize,
    pub(super) backend_lb: String,
}

pub(super) struct ForwardTargetResolutionInput<'a> {
    pub(super) request: TargetResolutionRequest<'a>,
    pub(super) context: ResolutionContext<'a>,
    pub(super) observation: ResolutionObservation<'a>,
}

pub(in crate::quic_listener) struct BootstrapTargetResolution {
    pub(in crate::quic_listener) upstream_name: String,
    pub(in crate::quic_listener) upstream_pool: Arc<RwLock<UpstreamPool>>,
    pub(in crate::quic_listener) upstream_policy: RuntimeUpstreamPolicy,
    pub(in crate::quic_listener) backend_addr: String,
    pub(in crate::quic_listener) backend_index: usize,
}

pub(in crate::quic_listener) struct BootstrapTargetResolutionInput<'a> {
    pub(in crate::quic_listener) request: TargetResolutionRequest<'a>,
    pub(in crate::quic_listener) context: ResolutionContext<'a>,
    pub(in crate::quic_listener) observation: ResolutionObservation<'a>,
}

struct BackendSelectionPlan {
    lb_type: String,
    lb_key: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteResolutionFailureKind {
    NoRoute,
    MissingPool,
    PoolLockPoisoned,
    NoServers,
    NoHealthyServers,
    InvalidServerAddress,
    OtherTransport,
    Other,
}

impl QUICListener {
    fn classify_route_resolution_transport_reason(reason: &str) -> RouteResolutionFailureKind {
        if reason.starts_with("no route for ") {
            return RouteResolutionFailureKind::NoRoute;
        }
        if reason.starts_with("pool not found:") {
            return RouteResolutionFailureKind::MissingPool;
        }
        if reason == "upstream pool lock poisoned" {
            return RouteResolutionFailureKind::PoolLockPoisoned;
        }
        if reason == "no servers in upstream" {
            return RouteResolutionFailureKind::NoServers;
        }
        if reason == "no healthy servers" {
            return RouteResolutionFailureKind::NoHealthyServers;
        }
        if reason == "invalid server address" {
            return RouteResolutionFailureKind::InvalidServerAddress;
        }
        RouteResolutionFailureKind::OtherTransport
    }

    fn classify_route_resolution_failure(err: &ProxyError) -> RouteResolutionFailureKind {
        match err {
            ProxyError::Transport(reason) => {
                Self::classify_route_resolution_transport_reason(reason)
            }
            _ => RouteResolutionFailureKind::Other,
        }
    }

    fn log_route_resolution_failure(request: &TargetResolutionRequest<'_>, err: &ProxyError) {
        let authority = request.authority.unwrap_or("-");
        let failure_kind = Self::classify_route_resolution_failure(err);
        let message = format!(
            "route/backend resolution failed method={} path={} authority={} kind={:?}: {}",
            request.method, request.path, authority, failure_kind, err
        );
        match failure_kind {
            RouteResolutionFailureKind::NoRoute => debug!("{}", message),
            _ => warn!("{}", message),
        }
    }

    fn observe_route_resolution_failure(
        request: &TargetResolutionRequest<'_>,
        err: &ProxyError,
        metrics: &Metrics,
        elapsed: Duration,
    ) {
        let _ = observe_proxy_error_outcome(
            metrics,
            RouteOutcomeTarget::UNROUTED,
            None,
            elapsed,
            None,
            err,
            None,
        );
        Self::log_route_resolution_failure(request, err);
    }

    pub(in crate::quic_listener) fn bootstrap_route_resolution_error_response(
        err: &ProxyError,
    ) -> (http::StatusCode, &'static [u8]) {
        match Self::classify_route_resolution_failure(err) {
            RouteResolutionFailureKind::NoRoute => (http::StatusCode::BAD_GATEWAY, b"no route\n"),
            RouteResolutionFailureKind::MissingPool => {
                (http::StatusCode::BAD_GATEWAY, b"no pool\n")
            }
            RouteResolutionFailureKind::PoolLockPoisoned => {
                (http::StatusCode::BAD_GATEWAY, b"pool error\n")
            }
            RouteResolutionFailureKind::NoServers
            | RouteResolutionFailureKind::InvalidServerAddress => {
                (http::StatusCode::SERVICE_UNAVAILABLE, b"no backends\n")
            }
            RouteResolutionFailureKind::NoHealthyServers => (
                http::StatusCode::SERVICE_UNAVAILABLE,
                b"no healthy backends\n",
            ),
            RouteResolutionFailureKind::OtherTransport | RouteResolutionFailureKind::Other => (
                http::StatusCode::BAD_GATEWAY,
                b"route/backend resolution failed\n",
            ),
        }
    }

    pub(super) fn resolve_forwarding_target(
        input: ForwardTargetResolutionInput<'_>,
    ) -> Result<ForwardTargetResolution, ProxyError> {
        let ForwardTargetResolutionInput {
            request,
            context,
            observation,
        } = input;
        let TargetResolution { route, backend } =
            match Self::resolve_backend_without_inflight_request(&request, &context) {
                Ok(resolved) => resolved,
                Err(err) => {
                    Self::observe_route_resolution_failure(
                        &request,
                        &err,
                        observation.metrics,
                        observation.elapsed,
                    );
                    return Err(err);
                }
            };
        let RouteResolution {
            upstream_name,
            upstream_pool,
            upstream_policy,
            route_path_len,
            route_host_specific,
            route_reason,
        } = route;
        let BackendSelection {
            backend_addr,
            backend_index,
            backend_lb,
        } = backend;

        Ok(ForwardTargetResolution {
            upstream_name,
            upstream_pool,
            upstream_policy,
            route_path_len,
            route_host_specific,
            route_reason: format!("{route_reason:?}"),
            backend_addr,
            backend_index,
            backend_lb,
        })
    }

    pub(in crate::quic_listener) fn resolve_bootstrap_target(
        input: BootstrapTargetResolutionInput<'_>,
    ) -> Result<BootstrapTargetResolution, ProxyError> {
        let BootstrapTargetResolutionInput {
            request,
            context,
            observation,
        } = input;
        let TargetResolution { route, backend } =
            match Self::resolve_backend_internal(&request, &context, true) {
                Ok(resolved) => resolved,
                Err(err) => {
                    Self::observe_route_resolution_failure(
                        &request,
                        &err,
                        observation.metrics,
                        observation.elapsed,
                    );
                    return Err(err);
                }
            };

        Ok(BootstrapTargetResolution {
            upstream_name: route.upstream_name,
            upstream_pool: route.upstream_pool,
            upstream_policy: route.upstream_policy,
            backend_addr: backend.backend_addr,
            backend_index: backend.backend_index,
        })
    }

    #[allow(clippy::type_complexity)]
    fn resolve_route_target(
        request: &TargetResolutionRequest<'_>,
        context: &ResolutionContext<'_>,
    ) -> Result<RouteResolution, ProxyError> {
        if request.method.is_empty() || request.path.is_empty() {
            return Err(ProxyError::Transport("empty method or path".into()));
        }

        let route_decision = context
            .routing_index
            .lookup_with_decision_for_method(request.path, request.authority, Some(request.method))
            .ok_or_else(|| ProxyError::Transport(format!("no route for {}", request.path)))?;
        let upstream_name = route_decision.upstream.to_string();
        let upstream_pool = context
            .upstream_pools
            .get(route_decision.upstream)
            .ok_or_else(|| ProxyError::Transport(format!("pool not found: {upstream_name}")))?
            .clone();
        let upstream_policy = context
            .upstream_policies
            .get(route_decision.upstream)
            .cloned()
            .unwrap_or_default();

        Ok(RouteResolution {
            upstream_name,
            upstream_pool,
            upstream_policy,
            route_path_len: route_decision.matched_path_len,
            route_host_specific: route_decision.host_specific,
            route_reason: route_decision.reason,
        })
    }

    fn build_backend_selection_plan(
        request: &TargetResolutionRequest<'_>,
        pool: &UpstreamPool,
    ) -> BackendSelectionPlan {
        let ResolvedLbKey {
            value: lb_key,
            source: _lb_key_source,
        } = Self::resolve_lb_key_for_runtime_request(
            pool.lb_strategy(),
            pool.lb_key_spec(),
            request,
        );
        BackendSelectionPlan {
            lb_type: pool.lb_strategy().canonical_name().to_string(),
            lb_key,
        }
    }

    fn no_servers_in_upstream_error() -> ProxyError {
        ProxyError::Transport("no servers in upstream".into())
    }

    fn no_healthy_servers_error(pool: &UpstreamPool) -> ProxyError {
        let summary = pool.membership_summary();
        error!(
            "no healthy backends available: {}/{} backends healthy",
            summary.healthy_backends, summary.total_backends
        );
        ProxyError::Transport("no healthy servers".into())
    }

    fn select_backend_with_write_lock(
        pool: &mut UpstreamPool,
        plan: &BackendSelectionPlan,
        begin_request: bool,
    ) -> Result<BackendSelection, ProxyError> {
        let idx = if begin_request {
            pool.pick(plan.lb_key.as_str())
        } else {
            pool.pick_without_begin(plan.lb_key.as_str())
        }
        .ok_or_else(|| Self::no_healthy_servers_error(pool))?;
        let backend_addr = pool
            .backend_address(idx)
            .map(str::to_string)
            .ok_or_else(|| ProxyError::Transport("invalid server address".into()))?;
        Ok(BackendSelection {
            backend_addr,
            backend_index: idx,
            backend_lb: plan.lb_type.clone(),
        })
    }

    fn select_backend_from_pool(
        request: &TargetResolutionRequest<'_>,
        upstream_pool: &Arc<RwLock<UpstreamPool>>,
        begin_request: bool,
    ) -> Result<BackendSelection, ProxyError> {
        let mut pool = upstream_pool
            .write()
            .map_err(|_| ProxyError::Transport("upstream pool lock poisoned".into()))?;
        if pool.is_empty() {
            return Err(Self::no_servers_in_upstream_error());
        }
        let plan = Self::build_backend_selection_plan(request, &pool);
        Self::select_backend_with_write_lock(&mut pool, &plan, begin_request)
    }

    fn log_backend_selection(
        request: &TargetResolutionRequest<'_>,
        backend_addr: &str,
        lb_type: &str,
        upstream_name: &str,
        route_path_len: usize,
        route_host_specific: bool,
        route_reason: &RouteDecisionReason,
    ) {
        debug!(
            "Resolved backend method={} path={} authority={} route={} backend={} via={} path_len={} host_specific={} reason={:?}",
            request.method,
            request.path,
            request.authority.unwrap_or("-"),
            upstream_name,
            backend_addr,
            lb_type,
            route_path_len,
            route_host_specific,
            route_reason
        );
    }

    fn resolve_backend_internal(
        request: &TargetResolutionRequest<'_>,
        context: &ResolutionContext<'_>,
        begin_request: bool,
    ) -> Result<TargetResolution, ProxyError> {
        let route = Self::resolve_route_target(request, context)?;
        let backend = Self::select_backend_from_pool(request, &route.upstream_pool, begin_request)?;

        Self::log_backend_selection(
            request,
            &backend.backend_addr,
            &backend.backend_lb,
            &route.upstream_name,
            route.route_path_len,
            route.route_host_specific,
            &route.route_reason,
        );
        Ok(TargetResolution { route, backend })
    }

    fn resolve_backend_without_inflight_request(
        request: &TargetResolutionRequest<'_>,
        context: &ResolutionContext<'_>,
    ) -> Result<TargetResolution, ProxyError> {
        Self::resolve_backend_internal(request, context, false)
    }

    #[cfg(test)]
    pub(in crate::quic_listener) fn resolve_backend_request_for_test(
        request: &TargetResolutionRequest<'_>,
        upstream_pools: &HashMap<String, Arc<RwLock<UpstreamPool>>>,
        upstream_policies: &HashMap<String, RuntimeUpstreamPolicy>,
        routing_index: &RouteIndex,
    ) -> Result<TargetResolution, ProxyError> {
        let context = ResolutionContext::new(routing_index, upstream_pools, upstream_policies);
        Self::resolve_backend_internal(request, &context, true)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, RwLock},
    };

    use impulse_config::{
        config::{
            Backend, Config, ForwardedHeaderPolicy, Listen, LoadBalancing, Resilience, RouteAuth,
            RouteMatch, Tls, Upstream, UpstreamHostPolicy,
        },
        runtime::RuntimeConfig,
    };

    use super::*;
    use crate::routing::index::RouteIndex;

    fn upstream(
        path_prefix: &str,
        host: Option<&str>,
        method: Option<&str>,
        backend_addr: &str,
    ) -> Upstream {
        Upstream {
            load_balancing: LoadBalancing {
                lb_type: "round-robin".to_string(),
                key: None,
            },
            auth: RouteAuth::default(),
            host_policy: UpstreamHostPolicy::default(),
            forwarded_headers: ForwardedHeaderPolicy::default(),
            tls: None,
            route: RouteMatch {
                host: host.map(str::to_string),
                path_prefix: Some(path_prefix.to_string()),
                method: method.map(str::to_string),
            },
            backends: vec![Backend {
                id: "b1".to_string(),
                address: backend_addr.to_string(),
                weight: 1,
                health_check: None,
            }],
        }
    }

    fn runtime_config(upstreams: HashMap<String, Upstream>) -> RuntimeConfig {
        runtime_config_with_connect(upstreams, false)
    }

    fn runtime_config_with_connect(
        upstreams: HashMap<String, Upstream>,
        allow_connect: bool,
    ) -> RuntimeConfig {
        let mut resilience = Resilience::default();
        resilience.protocol.allow_connect = allow_connect;
        RuntimeConfig::from_config(&Config {
            version: 1,
            listen: Listen {
                protocol: "http1".to_string(),
                tls: Tls {
                    cert: "/tmp/test-cert.pem".to_string(),
                    key: "/tmp/test-key.pem".to_string(),
                    ..Tls::default()
                },
                ..Listen::default()
            },
            listeners: Vec::new(),
            upstream: upstreams,
            load_balancing: None,
            upstream_tls: Default::default(),
            secrets: Default::default(),
            log: Default::default(),
            performance: Default::default(),
            observability: Default::default(),
            resilience,
            security: Default::default(),
        })
        .expect("runtime config")
    }

    fn upstream_pools(runtime: &RuntimeConfig) -> HashMap<String, Arc<RwLock<UpstreamPool>>> {
        runtime
            .upstreams
            .iter()
            .map(|(name, upstream)| {
                (
                    name.clone(),
                    Arc::new(RwLock::new(
                        UpstreamPool::from_runtime_upstream(upstream).expect("pool"),
                    )),
                )
            })
            .collect()
    }

    #[test]
    fn resolve_route_target_prefers_method_specific_host_route_in_overlapping_scenarios() {
        let runtime = runtime_config(HashMap::from([
            (
                "default".to_string(),
                upstream("/api", None, None, "http://127.0.0.1:7001"),
            ),
            (
                "host_only".to_string(),
                upstream(
                    "/api",
                    Some("pay.example.com"),
                    None,
                    "http://127.0.0.1:7002",
                ),
            ),
            (
                "method_host".to_string(),
                upstream(
                    "/api",
                    Some("pay.example.com"),
                    Some("POST"),
                    "http://127.0.0.1:7003",
                ),
            ),
        ]));
        let routing_index = RouteIndex::from_runtime_upstreams(&runtime.upstreams);
        let pools = upstream_pools(&runtime);
        let policies = runtime
            .upstreams
            .iter()
            .map(|(name, upstream)| (name.clone(), upstream.policy.clone()))
            .collect::<HashMap<_, _>>();
        let context = ResolutionContext::new(&routing_index, &pools, &policies);
        let request = TargetResolutionRequest::new(
            "POST",
            "/api/orders",
            Some("pay.example.com"),
            None,
            None,
        );

        let route = QUICListener::resolve_route_target(&request, &context).expect("resolved route");

        assert_eq!(route.upstream_name, "method_host");
        assert!(route.route_host_specific);
        assert_eq!(route.route_path_len, "/api".len());
        assert_eq!(
            route.route_reason,
            RouteDecisionReason::MethodSpecificTieBreak
        );
    }

    #[test]
    fn resolve_route_target_returns_stable_unrouted_error_and_bootstrap_mapping() {
        let runtime = runtime_config(HashMap::from([(
            "api".to_string(),
            upstream(
                "/api",
                Some("api.example.com"),
                None,
                "http://127.0.0.1:7001",
            ),
        )]));
        let routing_index = RouteIndex::from_runtime_upstreams(&runtime.upstreams);
        let pools = upstream_pools(&runtime);
        let policies = runtime
            .upstreams
            .iter()
            .map(|(name, upstream)| (name.clone(), upstream.policy.clone()))
            .collect::<HashMap<_, _>>();
        let context = ResolutionContext::new(&routing_index, &pools, &policies);

        let request = TargetResolutionRequest::new(
            "GET",
            "/missing",
            Some("unknown.example.com"),
            None,
            None,
        );
        let first = match QUICListener::resolve_route_target(&request, &context) {
            Err(err) => err,
            Ok(_) => panic!("expected unrouted resolution failure"),
        };
        let second = match QUICListener::resolve_route_target(&request, &context) {
            Err(err) => err,
            Ok(_) => panic!("expected unrouted resolution failure"),
        };

        assert_eq!(first.to_string(), second.to_string());
        assert_eq!(first.to_string(), "transport error: no route for /missing");

        let (status, body) = QUICListener::bootstrap_route_resolution_error_response(&first);
        assert_eq!(status, http::StatusCode::BAD_GATEWAY);
        assert_eq!(body, b"no route\n");
    }

    #[test]
    fn resolve_forwarding_target_keeps_connect_method_for_websocket_routes() {
        let runtime = runtime_config_with_connect(
            HashMap::from([
                (
                    "websocket_get".to_string(),
                    upstream(
                        "/chat",
                        Some("ws.example.com"),
                        Some("GET"),
                        "http://127.0.0.1:7001",
                    ),
                ),
                (
                    "websocket_connect".to_string(),
                    upstream(
                        "/chat",
                        Some("ws.example.com"),
                        Some("CONNECT"),
                        "http://127.0.0.1:7002",
                    ),
                ),
            ]),
            true,
        );
        let routing_index = RouteIndex::from_runtime_upstreams(&runtime.upstreams);
        let pools = upstream_pools(&runtime);
        let policies = runtime
            .upstreams
            .iter()
            .map(|(name, upstream)| (name.clone(), upstream.policy.clone()))
            .collect::<HashMap<_, _>>();
        let context = ResolutionContext::new(&routing_index, &pools, &policies);
        let request = TargetResolutionRequest::new(
            "CONNECT",
            "/chat/socket",
            Some("ws.example.com"),
            None,
            None,
        );
        let metrics = Metrics::new(1, vec!["route".to_string()]);

        let resolved = QUICListener::resolve_forwarding_target(ForwardTargetResolutionInput {
            request,
            context,
            observation: ResolutionObservation::new(&metrics, Duration::ZERO),
        })
        .expect("resolved target");

        assert_eq!(resolved.upstream_name, "websocket_connect");
        assert_eq!(resolved.backend_addr, "http://127.0.0.1:7002");
    }

    #[test]
    fn bootstrap_route_resolution_error_response_maps_missing_pool_and_empty_request_stably() {
        let missing_pool = ProxyError::Transport("pool not found: payments".to_string());
        let (status, body) = QUICListener::bootstrap_route_resolution_error_response(&missing_pool);
        assert_eq!(status, http::StatusCode::BAD_GATEWAY);
        assert_eq!(body, b"no pool\n");

        let other = ProxyError::Transport("empty method or path".to_string());
        let (status, body) = QUICListener::bootstrap_route_resolution_error_response(&other);
        assert_eq!(status, http::StatusCode::BAD_GATEWAY);
        assert_eq!(body, b"route/backend resolution failed\n");
    }
}
