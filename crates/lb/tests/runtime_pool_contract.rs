//! Runtime-upstream pool contract tests.

use std::collections::HashMap;

use spooky_config::{
    config::{Backend, Config, HealthCheck, Listen, RouteMatch, Tls},
    runtime::RuntimeConfig,
};
use spooky_lb::upstream_pool::UpstreamPool;

#[test]
fn runtime_upstream_builds_a_pool_with_canonical_lb_contract() {
    let upstream = spooky_config::config::Upstream {
        load_balancing: spooky_config::config::LoadBalancing {
            lb_type: "round-robin".to_string(),
            key: None,
        },
        auth: Default::default(),
        host_policy: Default::default(),
        forwarded_headers: Default::default(),
        tls: None,
        route: RouteMatch {
            path_prefix: Some("/".to_string()),
            ..Default::default()
        },
        backends: vec![
            Backend {
                id: "backend1".to_string(),
                address: "127.0.0.1:8001".to_string(),
                weight: 100,
                health_check: Some(HealthCheck {
                    path: "/health".to_string(),
                    interval: 5000,
                    timeout_ms: 2000,
                    failure_threshold: 3,
                    success_threshold: 2,
                    cooldown_ms: 10000,
                }),
            },
            Backend {
                id: "backend2".to_string(),
                address: "127.0.0.1:8002".to_string(),
                weight: 200,
                health_check: Some(HealthCheck {
                    path: "/health".to_string(),
                    interval: 5000,
                    timeout_ms: 2000,
                    failure_threshold: 3,
                    success_threshold: 2,
                    cooldown_ms: 10000,
                }),
            },
        ],
    };

    let mut upstreams = HashMap::new();
    upstreams.insert("api".to_string(), upstream);
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
        log: Default::default(),
        performance: Default::default(),
        observability: Default::default(),
        resilience: Default::default(),
        security: Default::default(),
    })
    .unwrap();

    let upstream_pool =
        UpstreamPool::from_runtime_upstream(runtime.upstreams.get("api").unwrap()).unwrap();
    assert_eq!(upstream_pool.load_balancer_name(), "round-robin");
    assert_eq!(upstream_pool.backend_count(), 2);
    assert_eq!(upstream_pool.backend_address(0), Some("127.0.0.1:8001"));
    assert_eq!(upstream_pool.backend_address(1), Some("127.0.0.1:8002"));
}
