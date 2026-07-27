//! Load-balancing strategy domain tests.

use std::{collections::HashMap, time::Duration};

use spooky_config::{
    config::{Backend, Config, HealthCheck, Listen, LoadBalancing as ConfigLoadBalancing, RouteMatch, Tls, Upstream},
    runtime::RuntimeConfig,
};
use spooky_lb::{load_balancing::LoadBalancing, upstream_pool::UpstreamPool};

#[test]
fn supported_strategy_names_normalize_through_the_canonical_facade() {
    assert!(LoadBalancing::from_config("round-robin").is_ok());
    assert!(LoadBalancing::from_config("consistent-hash").is_ok());
    assert!(LoadBalancing::from_config("random").is_ok());
    assert!(LoadBalancing::from_config("least-connections").is_ok());
    assert!(LoadBalancing::from_config("latency-aware").is_ok());
    assert!(LoadBalancing::from_config("sticky-cid").is_ok());
    assert!(LoadBalancing::from_config("unknown").is_err());
}

fn runtime_upstream(strategy: &str, backend_count: usize) -> spooky_config::runtime::RuntimeUpstream {
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
            backends: (0..backend_count)
                .map(|index| Backend {
                    id: format!("backend-{index}"),
                    address: format!("http://127.0.0.1:{}", 7001 + index),
                    weight: 1,
                    health_check: Some(HealthCheck {
                        path: "/health".to_string(),
                        interval: 1,
                        timeout_ms: 1000,
                        failure_threshold: 1,
                        success_threshold: 1,
                        cooldown_ms: 0,
                    }),
                })
                .collect(),
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

fn pool(strategy: &str, backend_count: usize) -> UpstreamPool {
    UpstreamPool::from_runtime_upstream(&runtime_upstream(strategy, backend_count)).expect("pool")
}

#[test]
fn round_robin_sequences_across_healthy_backends() {
    let mut pool = pool("round-robin", 3);

    let picks: Vec<_> = (0..6)
        .map(|_| pool.pick_without_begin("ignored").expect("round-robin pick"))
        .collect();

    assert_eq!(picks, vec![0, 1, 2, 0, 1, 2]);
}

#[test]
fn random_selection_stays_within_healthy_membership() {
    let mut pool = pool("random", 3);
    let _ = pool.mark_backend_failure_from_active_check(0);

    let picks: Vec<_> = (0..64)
        .map(|_| pool.pick_without_begin("ignored").expect("random pick"))
        .collect();

    assert!(
        picks.iter().all(|pick| matches!(pick, 1 | 2)),
        "random strategy must only return healthy backend indices"
    );
}

#[test]
fn consistent_hash_is_sticky_while_membership_is_stable() {
    let mut pool = pool("consistent-hash", 3);

    let first = pool
        .pick_without_begin("tenant:alpha")
        .expect("first consistent-hash pick");
    let repeated: Vec<_> = (0..8)
        .map(|_| {
            pool.pick_without_begin("tenant:alpha")
                .expect("repeated consistent-hash pick")
        })
        .collect();

    assert!(
        repeated.iter().all(|pick| *pick == first),
        "consistent hash should remain sticky for the same key while membership is unchanged"
    );
}

#[test]
fn least_connections_prefers_the_backend_with_fewer_active_requests() {
    let mut pool = pool("least-connections", 3);
    pool.begin_request_for_accounting(0);
    pool.begin_request_for_accounting(0);
    pool.begin_request_for_accounting(1);

    let pick = pool
        .pick_without_begin("ignored")
        .expect("least-connections pick");

    assert_eq!(pick, 2);
}

#[test]
fn latency_aware_prefers_the_backend_with_lower_latency() {
    let mut pool = pool("latency-aware", 2);
    pool.finish_request(0, Duration::from_millis(150), Some(200));
    pool.finish_request(1, Duration::from_millis(20), Some(200));

    let pick = pool
        .pick_without_begin("ignored")
        .expect("latency-aware pick");

    assert_eq!(pick, 1);
}

#[test]
fn strategies_return_none_when_all_backends_are_unhealthy() {
    for strategy in [
        "round-robin",
        "random",
        "consistent-hash",
        "least-connections",
        "latency-aware",
    ] {
        let mut pool = pool(strategy, 2);
        let _ = pool.mark_backend_failure_from_active_check(0);
        let _ = pool.mark_backend_failure_from_active_check(1);

        assert!(
            pool.pick_without_begin("key").is_none(),
            "strategy {strategy} should not select a backend when every backend is unhealthy"
        );
    }
}
