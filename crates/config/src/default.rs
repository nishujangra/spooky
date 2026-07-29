use crate::config::{
    ApiKeyAuth, ControlApi, CURRENT_CONFIG_VERSION, HealthCheck, Listen, LoadBalancing, Log,
    MetricsEndpoint, RoutingTransparency, Tracing, JwtAuth,
};

// default values
pub fn get_default_version() -> u32 {
    CURRENT_CONFIG_VERSION
}

pub fn get_default_load_balancing() -> LoadBalancing {
    LoadBalancing {
        lb_type: String::from("round-robin"),
        key: None,
    }
}

pub fn auth_default_external_timeout_ms() -> u64 {
    1_000
}

pub fn upstream_tls_default_verify_certificates() -> bool {
    true
}

pub fn upstream_tls_default_strict_sni() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documented_scalar_defaults_match_contract() {
        assert_eq!(get_default_version(), 1);
        assert_eq!(Listen::default().protocol, "http3");
        assert_eq!(Listen::default().port, 9889);
        assert_eq!(Listen::default().address, "0.0.0.0");
        assert_eq!(HealthCheck::default().path, "/health");
        assert_eq!(HealthCheck::default().interval, 5_000);
        assert_eq!(HealthCheck::default().timeout_ms, 1_000);
        assert_eq!(HealthCheck::default().failure_threshold, 3);
        assert_eq!(HealthCheck::default().success_threshold, 2);
        assert_eq!(ApiKeyAuth::default().header_name, "x-api-key");
        assert_eq!(JwtAuth::default().clock_skew_secs, 30);
        assert_eq!(MetricsEndpoint::default().port, 9901);
        assert_eq!(ControlApi::default().port, 9902);
        assert_eq!(Tracing::default().sample_ratio, 1.0);
        assert!(RoutingTransparency::default().include_reason);
        assert_eq!(crate::config::ScopedRateLimit::default_idle_ttl_secs(), 300);
        assert_eq!(crate::config::Watchdog::default().check_interval_ms, 1_000);
        assert_eq!(crate::config::Resilience::default().retry_budget.ratio_percent, 10);
    }

    #[test]
    fn documented_composite_defaults_match_contract() {
        let load_balancing = get_default_load_balancing();
        assert_eq!(load_balancing.lb_type, "round-robin");
        assert_eq!(load_balancing.key, None);

        let log = Log::default();
        assert_eq!(log.level, "info");
        assert!(!log.file.enabled);
        assert_eq!(log.file.path, "/var/log/spooky/spooky.log");
        assert_eq!(log.format, crate::config::LogFormat::Plain);

        let metrics = MetricsEndpoint::default();
        assert_eq!(metrics.address, "127.0.0.1");
        assert_eq!(metrics.path, "/metrics");
        assert_eq!(metrics.max_connections, 512);
        assert_eq!(metrics.connection_timeout_ms, 30_000);

        let control_api = ControlApi::default();
        assert_eq!(control_api.address, "127.0.0.1");
        assert_eq!(control_api.health_path, "/health");
        assert_eq!(control_api.ready_path, "/ready");
        assert_eq!(control_api.runtime_path, "/admin/runtime");
        assert_eq!(control_api.restart_path, "/admin/runtime/restart");
        assert_eq!(control_api.reload_path, "/admin/runtime/reload");
        assert_eq!(control_api.reload_certs_path, "/admin/runtime/reload-certs");
        assert_eq!(control_api.max_connections, 256);
        assert_eq!(control_api.connection_timeout_ms, 30_000);

        let tracing = Tracing::default();
        assert_eq!(tracing.service_name, "spooky");
        assert_eq!(tracing.otlp_endpoint, None);

        let routing = RoutingTransparency::default();
        assert!(!routing.enabled);
        assert!(!routing.expose_header);
        assert_eq!(routing.header_name, "x-spooky-route-decision");

        let resilience = crate::config::Resilience::default();
        assert_eq!(resilience.adaptive_admission.min_limit, 64);
        assert_eq!(resilience.route_queue.default_cap, 512);
        assert_eq!(resilience.protocol.max_headers_bytes, 16 * 1024);
        assert_eq!(resilience.circuit_breaker.failure_threshold, 3);
        assert_eq!(resilience.hedging.delay_ms, 100);
        assert_eq!(resilience.brownout.recover_inflight_percent, 60);

        let watchdog = crate::config::Watchdog::default();
        assert_eq!(watchdog.poll_stall_timeout_ms, 5_000);
        assert_eq!(watchdog.timeout_error_rate_percent, 60);
    }
}
