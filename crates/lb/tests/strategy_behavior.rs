//! Load-balancing strategy domain tests.

mod common;

use std::time::Duration;

use spooky_lb::load_balancing::LoadBalancing;
use common::pool;

#[test]
fn load_balancing_facade_normalizes_supported_strategy_names() {
    assert!(LoadBalancing::from_config("round-robin").is_ok());
    assert!(LoadBalancing::from_config("consistent-hash").is_ok());
    assert!(LoadBalancing::from_config("random").is_ok());
    assert!(LoadBalancing::from_config("least-connections").is_ok());
    assert!(LoadBalancing::from_config("latency-aware").is_ok());
    assert!(LoadBalancing::from_config("sticky-cid").is_ok());
    assert!(LoadBalancing::from_config("unknown").is_err());
}

#[test]
fn round_robin_cycles_across_healthy_backends() {
    let mut pool = pool("round-robin", 3);

    let picks: Vec<_> = (0..6)
        .map(|_| {
            pool.pick_without_begin("ignored")
                .expect("round-robin pick")
        })
        .collect();

    assert_eq!(picks, vec![0, 1, 2, 0, 1, 2]);
}

#[test]
fn round_robin_keeps_alternating_when_request_keys_vary() {
    let mut pool = pool("round-robin", 2);

    let keys = [
        "tenant-a", "tenant-b", "tenant-c", "tenant-d", "tenant-e", "tenant-f",
    ];
    let picks: Vec<_> = keys
        .into_iter()
        .map(|key| {
            pool.pick_without_begin(key)
                .expect("round-robin regression pick")
        })
        .collect();

    assert_eq!(
        picks,
        vec![0, 1, 0, 1, 0, 1],
        "round-robin must keep alternating across healthy backends even when caller keys vary"
    );
}

#[test]
fn random_strategy_only_selects_healthy_membership() {
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
fn consistent_hash_remains_sticky_while_membership_is_stable() {
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
fn least_connections_prefers_the_backend_with_fewer_inflight_requests() {
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
fn latency_aware_prefers_the_backend_with_lower_observed_latency() {
    let mut pool = pool("latency-aware", 2);
    pool.finish_request(0, Duration::from_millis(150), Some(200));
    pool.finish_request(1, Duration::from_millis(20), Some(200));

    let pick = pool
        .pick_without_begin("ignored")
        .expect("latency-aware pick");

    assert_eq!(pick, 1);
}

#[test]
fn strategies_return_no_backend_when_every_backend_is_unhealthy() {
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
