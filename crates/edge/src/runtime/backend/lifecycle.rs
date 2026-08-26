mod coordinator;
mod dns;
mod health;

pub(crate) use self::{
    coordinator::BackendLifecycleCoordinator, coordinator::RuntimeBackendLifecycleState,
    dns::BackendRefreshClassification,
};
pub(crate) use self::{
    dns::{BackendDnsRefreshApplication, log_backend_dns_refresh, observe_backend_dns_refresh},
    health::{
        ActiveHealthCheckEvaluation, apply_backend_health_observation,
        apply_backend_request_accounting, apply_backend_request_feedback,
        evaluate_active_health_check,
    },
};

#[cfg(test)]
mod test_support {
    use std::{
        collections::HashMap,
        sync::{Arc, RwLock},
        time::Duration,
    };

    use impulse_config::{
        config::{Backend, Config, HealthCheck, Listen, LoadBalancing, RouteMatch, Tls, Upstream},
        runtime::{RuntimeBackendTransportKind, RuntimeConfig},
    };
    use impulse_lb::upstream_pool::UpstreamPool;
    use impulse_transport::{SharedDnsResolver, UpstreamTransportPool};

    pub(crate) fn test_upstream_pool_with_interval(interval: u64) -> Arc<RwLock<UpstreamPool>> {
        let mut upstreams = HashMap::new();
        upstreams.insert(
            "api".to_string(),
            Upstream {
                tls: None,
                load_balancing: LoadBalancing {
                    lb_type: "round-robin".to_string(),
                    key: None,
                },
                auth: Default::default(),
                host_policy: Default::default(),
                forwarded_headers: Default::default(),
                route: RouteMatch {
                    host: Some("api.example.com".to_string()),
                    path_prefix: None,
                    method: None,
                },
                backends: vec![Backend {
                    id: "backend-a".to_string(),
                    address: "127.0.0.1:8080".to_string(),
                    weight: 1,
                    health_check: (interval > 0).then_some(HealthCheck {
                        path: "/health".to_string(),
                        interval,
                        timeout_ms: 1000,
                        failure_threshold: 1,
                        success_threshold: 1,
                        cooldown_ms: 0,
                    }),
                }],
            },
        );

        let runtime = RuntimeConfig::from_config(&Config {
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
            resilience: Default::default(),
            security: Default::default(),
        })
        .expect("runtime config");

        Arc::new(RwLock::new(
            UpstreamPool::from_runtime_upstream(runtime.upstreams.get("api").expect("upstream"))
                .expect("pool"),
        ))
    }

    pub(crate) fn test_upstream_pool() -> Arc<RwLock<UpstreamPool>> {
        test_upstream_pool_with_interval(0)
    }

    pub(crate) fn test_active_health_upstream_pool() -> Arc<RwLock<UpstreamPool>> {
        test_upstream_pool_with_interval(1000)
    }

    pub(crate) fn test_transport_pool(backend_addr: &str) -> UpstreamTransportPool {
        UpstreamTransportPool::new_from_runtime_backends(
            [(backend_addr.to_string(), RuntimeBackendTransportKind::Http1)],
            HashMap::new(),
            impulse_config::runtime::RuntimeBackendConnectionPolicy {
                max_inflight: 32,
                max_idle_per_backend: 8,
                pool_idle_timeout: Duration::from_secs(30),
                connect_timeout: Duration::from_secs(2),
                execution_timeout: Duration::from_secs(5),
            },
            SharedDnsResolver::new(),
        )
        .expect("transport pool")
    }
}
