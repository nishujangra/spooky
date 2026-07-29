use std::collections::HashMap;

use spooky_config::{
    config::{
        Backend, Config, HealthCheck, Listen, LoadBalancing as ConfigLoadBalancing, RouteMatch,
        Tls, Upstream,
    },
    runtime::{RuntimeConfig, RuntimeUpstream},
};
use spooky_lb::upstream_pool::UpstreamPool;

pub fn runtime_upstream(strategy: &str, backends: Vec<Backend>) -> RuntimeUpstream {
    let mut upstreams = HashMap::new();
    upstreams.insert(
        "api".to_string(),
        Upstream {
            tls: None,
            load_balancing: ConfigLoadBalancing {
                lb_type: strategy.to_string(),
                key: None,
            },
            auth: Default::default(),
            host_policy: Default::default(),
            forwarded_headers: Default::default(),
            route: RouteMatch::default(),
            backends,
        },
    );

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
        log: Default::default(),
        performance: Default::default(),
        observability: Default::default(),
        resilience: Default::default(),
        security: Default::default(),
    })
    .expect("runtime config")
    .upstreams
    .remove("api")
    .expect("runtime upstream")
}

#[allow(dead_code)]
pub fn test_backend(id: impl Into<String>, address: impl Into<String>) -> Backend {
    Backend {
        id: id.into(),
        address: address.into(),
        weight: 1,
        health_check: Some(HealthCheck {
            path: "/health".to_string(),
            interval: 1,
            timeout_ms: 1000,
            failure_threshold: 1,
            success_threshold: 1,
            cooldown_ms: 0,
        }),
    }
}

#[allow(dead_code)]
pub fn weighted_backend(
    id: impl Into<String>,
    address: impl Into<String>,
    weight: u32,
    health_check: HealthCheck,
) -> Backend {
    Backend {
        id: id.into(),
        address: address.into(),
        weight,
        health_check: Some(health_check),
    }
}

#[allow(dead_code)]
pub fn indexed_backends(backend_count: usize, base_port: u16) -> Vec<Backend> {
    (0..backend_count)
        .map(|index| {
            test_backend(
                format!("backend-{index}"),
                format!("http://127.0.0.1:{}", base_port + index as u16),
            )
        })
        .collect()
}

#[allow(dead_code)]
pub fn pool(strategy: &str, backend_count: usize) -> UpstreamPool {
    UpstreamPool::from_runtime_upstream(&runtime_upstream(
        strategy,
        indexed_backends(backend_count, 7001),
    ))
    .expect("pool")
}
