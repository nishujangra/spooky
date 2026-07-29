//! Default-lowering regressions for omitted config fields.

use std::time::Duration;

use spooky_config::{
    config::{
        CURRENT_CONFIG_VERSION, ControlApi, ExternalAuthFailureMode, ForwardedHeaderPolicyMode,
        LoadBalancing, MetricsEndpoint, Performance, Resilience, RoutingTransparency, Tracing,
        UpstreamHostPolicyMode, UpstreamTls,
    },
    runtime::RuntimeLoadBalancingStrategy,
};

use crate::common::{
    api_runtime_upstream, parse_config, primary_listener_runtime_config, runtime_config_from_yaml,
};

fn sparse_config_yaml() -> &'static str {
    r#"
listen:
  tls:
    cert: "/tmp/tls/default.pem"
    key: "/tmp/tls/default.key"
upstream:
  api:
    route:
      host: "api.example.com"
      path_prefix: "/"
    backends:
      - id: "api-1"
        address: "https://api.internal:8443"
"#
}

#[test]
fn omitted_fields_deserialize_to_the_documented_defaults() {
    let config = parse_config(sparse_config_yaml());
    let upstream = config.upstream.get("api").expect("api upstream");
    let backend = upstream.backends.first().expect("api backend");

    assert_eq!(config.version, CURRENT_CONFIG_VERSION);
    assert_eq!(config.listen.protocol, "http3");
    assert_eq!(config.listen.port, 9889);
    assert_eq!(config.listen.address, "0.0.0.0");
    assert!(config.listeners.is_empty());
    assert!(config.load_balancing.is_none());

    assert_eq!(
        config.upstream_tls.verify_certificates,
        UpstreamTls::default().verify_certificates
    );
    assert_eq!(
        config.upstream_tls.strict_sni,
        UpstreamTls::default().strict_sni
    );
    assert_eq!(config.upstream_tls.ca_file, None);
    assert_eq!(config.upstream_tls.ca_dir, None);

    assert_eq!(
        upstream.load_balancing.lb_type,
        LoadBalancing::default().lb_type
    );
    assert_eq!(upstream.load_balancing.key, LoadBalancing::default().key);
    assert_eq!(
        upstream.host_policy.mode,
        UpstreamHostPolicyMode::PassThrough
    );
    assert_eq!(upstream.host_policy.host, None);
    assert_eq!(
        upstream.forwarded_headers.mode,
        ForwardedHeaderPolicyMode::Overwrite
    );
    assert!(upstream.auth.api_key.is_none());
    assert!(upstream.auth.jwt.is_none());
    assert!(upstream.auth.external_auth.is_none());
    assert!(upstream.auth.required_scopes.is_empty());
    assert!(upstream.auth.required_roles.is_empty());
    assert!(upstream.tls.is_none());

    assert_eq!(backend.weight, 100);
    assert!(backend.health_check.is_none());

    assert_eq!(
        config.observability.metrics.port,
        MetricsEndpoint::default().port
    );
    assert_eq!(
        config.observability.control_api.port,
        ControlApi::default().port
    );
    assert_eq!(
        config.observability.tracing.service_name,
        Tracing::default().service_name
    );
    assert_eq!(
        config.observability.routing.header_name,
        RoutingTransparency::default().header_name
    );

    assert_eq!(
        config.performance.backend_timeout_ms,
        Performance::default().backend_timeout_ms
    );
    assert_eq!(
        config.performance.backend_connect_timeout_ms,
        Performance::default().backend_connect_timeout_ms
    );
    assert_eq!(
        config.performance.global_inflight_limit,
        Performance::default().global_inflight_limit
    );
    assert_eq!(
        config.performance.max_response_body_bytes,
        Performance::default().max_response_body_bytes
    );

    assert_eq!(
        config.resilience.protocol.allow_connect,
        Resilience::default().protocol.allow_connect
    );
    assert_eq!(
        config.resilience.route_queue.default_cap,
        Resilience::default().route_queue.default_cap
    );
    assert_eq!(
        config.resilience.retry_budget.ratio_percent,
        Resilience::default().retry_budget.ratio_percent
    );
    assert_eq!(
        config.resilience.watchdog.check_interval_ms,
        Resilience::default().watchdog.check_interval_ms
    );
}

#[test]
fn runtime_lowering_preserves_effective_defaults_for_sparse_configs() {
    let runtime = runtime_config_from_yaml(sparse_config_yaml());
    let listener = primary_listener_runtime_config(&runtime);
    let api = api_runtime_upstream(&runtime);

    assert_eq!(runtime.version, CURRENT_CONFIG_VERSION);
    assert_eq!(listener.listen.listen.protocol, "http3");
    assert_eq!(listener.listen.listen.port, 9889);
    assert_eq!(listener.listen.listen.address, "0.0.0.0");

    assert_eq!(
        listener.policies.timeouts.backend_request,
        Duration::from_millis(Performance::default().backend_timeout_ms)
    );
    assert_eq!(
        listener.policies.timeouts.backend_connect,
        Duration::from_millis(Performance::default().backend_connect_timeout_ms)
    );
    assert_eq!(
        listener
            .policies
            .transport
            .connection_limits
            .global_inflight,
        Performance::default().global_inflight_limit
    );
    assert_eq!(
        listener.policies.transport.quic_initial_max_data,
        Performance::default().quic_initial_max_data
    );
    assert_eq!(
        runtime.policies.admission.route_queue.default_cap,
        Resilience::default().route_queue.default_cap
    );
    assert_eq!(
        runtime.policies.admission.watchdog.check_interval,
        Duration::from_millis(Resilience::default().watchdog.check_interval_ms)
    );

    assert_eq!(
        api.load_balancing.strategy,
        RuntimeLoadBalancingStrategy::RoundRobin
    );
    assert_eq!(api.load_balancing.key, None);
    assert!(api.policy.upstream_auth.api_key.is_none());
    assert!(api.policy.upstream_auth.jwt.is_none());
    assert!(api.policy.upstream_auth.external_auth.is_none());
    assert!(api.policy.upstream_auth.required_scopes.is_empty());
    assert!(api.policy.upstream_auth.required_roles.is_empty());
    assert_eq!(api.policy.host.0.mode, UpstreamHostPolicyMode::PassThrough);
    assert_eq!(api.policy.host.0.host, None);
    assert_eq!(
        api.policy.forwarded_headers.0.mode,
        ForwardedHeaderPolicyMode::Overwrite
    );

    assert_eq!(
        api.effective_tls.verify_certificates,
        UpstreamTls::default().verify_certificates
    );
    assert_eq!(
        api.effective_tls.strict_sni,
        UpstreamTls::default().strict_sni
    );
    assert_eq!(api.backends[0].backend.weight, 100);
    assert!(api.backends[0].health_check.is_none());

    assert_eq!(
        runtime.observability.metrics.port,
        MetricsEndpoint::default().port
    );
    assert_eq!(
        runtime.observability.control_api.port,
        ControlApi::default().port
    );
    assert_eq!(
        runtime.observability.tracing.service_name,
        Tracing::default().service_name
    );
    assert_eq!(
        runtime.observability.routing.header_name,
        RoutingTransparency::default().header_name
    );
}

#[test]
fn external_auth_default_fields_do_not_drift_when_omitted() {
    let config = parse_config(
        r#"
listen:
  tls:
    cert: "/tmp/tls/default.pem"
    key: "/tmp/tls/default.key"
upstream:
  api:
    auth:
      external_auth:
        kind: http
        endpoint: "https://auth.internal/check"
    route:
      host: "api.example.com"
      path_prefix: "/"
    backends:
      - id: "api-1"
        address: "https://api.internal:8443"
"#,
    );

    match config.upstream["api"].auth.external_auth.as_ref() {
        Some(spooky_config::config::ExternalAuth::Http {
            timeout_ms,
            failure_mode,
            request_headers,
            response_header_allowlist,
            ..
        }) => {
            assert_eq!(*timeout_ms, 1_000);
            assert_eq!(*failure_mode, ExternalAuthFailureMode::FailClosed);
            assert!(request_headers.is_empty());
            assert!(response_header_allowlist.is_empty());
        }
        other => panic!("unexpected external auth shape: {:?}", other),
    }
}
