use std::collections::HashMap;

use impulse_config::{
    config::{Backend, Config, HealthCheck, Listen, LoadBalancing, RouteMatch, Tls, Upstream},
    runtime::{RuntimeConfig, RuntimeUpstream},
};
use impulse_lb::upstream_pool::UpstreamPool;

use crate::{
    benchmark::{connection::fast_iterations, runner::run_case_aggregate},
    report::BenchCase,
};

pub fn benchmark_lb(scale: usize) -> Result<Vec<BenchCase>, String> {
    let mut rr_pool = build_lb_pool(scale, "round-robin")?;
    let mut random_pool = build_lb_pool(scale, "random")?;
    let mut ch_pool = build_lb_pool(scale, "consistent-hash")?;

    let keys = [
        "user:1", "user:2", "user:3", "user:4", "user:5", "user:6", "user:7", "user:8",
    ];
    let mut ch_key_idx = 0usize;

    Ok(vec![
        run_case_aggregate(
            "micro",
            "lb_round_robin_pick",
            scale,
            fast_iterations(scale),
            || rr_pool.pick("ignored").unwrap_or(usize::MAX),
        ),
        run_case_aggregate(
            "micro",
            "lb_random_pick",
            scale,
            fast_iterations(scale),
            || random_pool.pick("ignored").unwrap_or(usize::MAX),
        ),
        run_case_aggregate(
            "micro",
            "lb_consistent_hash_pick",
            scale,
            lb_ch_iterations(scale),
            || {
                let key = keys[ch_key_idx & 7];
                ch_key_idx = ch_key_idx.wrapping_add(1);
                ch_pool.pick(key).unwrap_or(usize::MAX)
            },
        ),
    ])
}

fn build_lb_upstream(scale: usize, lb_type: &str) -> Upstream {
    let backends = (0..scale.max(1))
        .map(|idx| Backend {
            id: format!("backend-{idx:05}"),
            address: format!("127.0.0.1:{}", 10_000 + (idx % 50_000)),
            weight: 1,
            health_check: Some(HealthCheck {
                path: "/health".to_string(),
                interval: 5_000,
                timeout_ms: 1_000,
                failure_threshold: 3,
                success_threshold: 2,
                cooldown_ms: 5_000,
            }),
        })
        .collect();

    Upstream {
        load_balancing: LoadBalancing {
            lb_type: lb_type.to_string(),
            key: None,
        },
        auth: Default::default(),
        host_policy: Default::default(),
        forwarded_headers: Default::default(),
        tls: None,
        route: RouteMatch {
            host: None,
            path_prefix: Some("/".to_string()),
            method: None,
        },
        backends,
    }
}

fn build_runtime_lb_upstream(scale: usize, lb_type: &str) -> Result<RuntimeUpstream, String> {
    let mut upstreams = HashMap::new();
    upstreams.insert("bench".to_string(), build_lb_upstream(scale, lb_type));

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
        resilience: Default::default(),
        security: Default::default(),
    })
    .map_err(|err| format!("failed to normalize benchmark upstream '{lb_type}': {err}"))?
    .upstreams
    .remove("bench")
    .ok_or_else(|| "missing normalized benchmark upstream".to_string())
}

pub fn build_lb_pool(scale: usize, lb_type: &str) -> Result<UpstreamPool, String> {
    let runtime_upstream = build_runtime_lb_upstream(scale, lb_type)?;
    UpstreamPool::from_runtime_upstream(&runtime_upstream)
        .map_err(|err| format!("failed to build LB pool '{lb_type}' for scale {scale}: {err}"))
}

fn lb_ch_iterations(scale: usize) -> u64 {
    match scale {
        100 => 300_000,
        1_000 => 200_000,
        10_000 => 80_000,
        _ => 50_000,
    }
}
