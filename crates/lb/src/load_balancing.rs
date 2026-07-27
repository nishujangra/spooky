//! Canonical load-balancing strategy façade.
//!
//! Strategy-specific implementations stay internal; callers should choose and
//! query strategies through [`LoadBalancing`] instead of depending on
//! algorithm-specific state.

use crate::{
    algorithms::{
        consistent_hash::ConsistentHash, latency_aware::LatencyAware,
        least_connections::LeastConnections, random::Random, round_robin::RoundRobin,
        sticky_cid::StickyCid,
    },
    backend_pool::BackendPool,
    hash::DEFAULT_REPLICAS,
};

pub enum LoadBalancing {
    RoundRobin(RoundRobin),
    ConsistentHash(ConsistentHash),
    Random(Random),
    LeastConnections(LeastConnections),
    LatencyAware(LatencyAware),
    StickyCid(StickyCid),
}

impl LoadBalancing {
    pub fn from_config(value: &str) -> Result<Self, String> {
        let mode = value.trim().to_lowercase();
        match mode.as_str() {
            "round-robin" | "round_robin" | "rr" => Ok(Self::RoundRobin(RoundRobin::new())),
            "consistent-hash" | "consistent_hash" | "ch" => {
                Ok(Self::ConsistentHash(ConsistentHash::new(DEFAULT_REPLICAS)))
            }
            "random" => Ok(Self::Random(Random::new())),
            "least-connections" | "least_connections" | "lc" => {
                Ok(Self::LeastConnections(LeastConnections::new()))
            }
            "latency-aware" | "latency_aware" | "la" => Ok(Self::LatencyAware(LatencyAware::new())),
            "sticky-cid" | "sticky_cid" | "cid-sticky" | "cid_sticky" => {
                Ok(Self::StickyCid(StickyCid::new(DEFAULT_REPLICAS)))
            }
            _ => Err(format!("unsupported load balancing type: {value}")),
        }
    }

    pub fn pick(&mut self, key: &str, pool: &BackendPool) -> Option<usize> {
        match self {
            LoadBalancing::RoundRobin(rr) => rr.pick(pool),
            LoadBalancing::ConsistentHash(ch) => ch.pick(key, pool),
            LoadBalancing::Random(rand) => rand.pick(pool),
            LoadBalancing::LeastConnections(lc) => lc.pick(pool),
            LoadBalancing::LatencyAware(la) => la.pick(pool),
            LoadBalancing::StickyCid(sticky) => sticky.pick(key, pool),
        }
    }

    pub fn pick_readonly(&self, _key: &str, pool: &BackendPool) -> Option<usize> {
        match self {
            LoadBalancing::RoundRobin(rr) => rr.pick_readonly(pool),
            LoadBalancing::Random(rand) => rand.pick_readonly(pool),
            LoadBalancing::LeastConnections(lc) => lc.pick_readonly(pool),
            LoadBalancing::LatencyAware(la) => la.pick_readonly(pool),
            // ConsistentHash and StickyCid keep mutable ring caches.
            LoadBalancing::ConsistentHash(_) | LoadBalancing::StickyCid(_) => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            LoadBalancing::RoundRobin(_) => "round-robin",
            LoadBalancing::ConsistentHash(_) => "consistent-hash",
            LoadBalancing::Random(_) => "random",
            LoadBalancing::LeastConnections(_) => "least-connections",
            LoadBalancing::LatencyAware(_) => "latency-aware",
            LoadBalancing::StickyCid(_) => "sticky-cid",
        }
    }

    pub fn from_runtime_strategy(
        strategy: spooky_config::runtime::RuntimeLoadBalancingStrategy,
    ) -> Result<Self, String> {
        Self::from_config(strategy.canonical_name())
    }
}

#[cfg(test)]
mod tests {
    use spooky_config::config::{Backend, HealthCheck};

    use super::LoadBalancing;
    use crate::{backend::BackendState, backend_pool::BackendPool};

    fn pool() -> BackendPool {
        BackendPool::new_from_states(vec![
            BackendState::new(&Backend {
                id: "a".to_string(),
                address: "http://127.0.0.1:7001".to_string(),
                weight: 1,
                health_check: None,
            }),
            BackendState::new(&Backend {
                id: "b".to_string(),
                address: "http://127.0.0.1:7002".to_string(),
                weight: 1,
                health_check: None,
            }),
        ])
    }

    fn health_checked_backend(address: &str) -> BackendState {
        BackendState::new(&Backend {
            id: format!("backend-{address}"),
            address: address.to_string(),
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
    }

    #[test]
    fn pick_readonly_is_available_only_for_readonly_strategies() {
        let pool = pool();

        assert!(
            LoadBalancing::from_config("round-robin")
                .expect("round robin")
                .pick_readonly("ignored", &pool)
                .is_some()
        );
        assert!(
            LoadBalancing::from_config("random")
                .expect("random")
                .pick_readonly("ignored", &pool)
                .is_some()
        );
        assert!(
            LoadBalancing::from_config("least-connections")
                .expect("least")
                .pick_readonly("ignored", &pool)
                .is_some()
        );
        assert!(
            LoadBalancing::from_config("latency-aware")
                .expect("latency")
                .pick_readonly("ignored", &pool)
                .is_some()
        );
        assert!(
            LoadBalancing::from_config("consistent-hash")
                .expect("consistent")
                .pick_readonly("tenant=a", &pool)
                .is_none()
        );
        assert!(
            LoadBalancing::from_config("sticky-cid")
                .expect("sticky")
                .pick_readonly("cid-1", &pool)
                .is_none()
        );
    }

    #[test]
    fn every_strategy_returns_none_for_an_empty_pool() {
        let empty_pool = BackendPool::new_from_states(Vec::new());

        for strategy in [
            "round-robin",
            "consistent-hash",
            "random",
            "least-connections",
            "latency-aware",
            "sticky-cid",
        ] {
            let mut lb = LoadBalancing::from_config(strategy).expect("strategy");
            assert!(
                lb.pick("tenant:alpha", &empty_pool).is_none(),
                "strategy {strategy} should not select from an empty backend pool"
            );
        }
    }

    #[test]
    fn every_strategy_returns_none_when_all_backends_are_unhealthy() {
        let mut unhealthy_pool = BackendPool::new_from_states(vec![
            health_checked_backend("10.0.0.1:443"),
            health_checked_backend("10.0.0.2:443"),
        ]);
        let _ = unhealthy_pool.mark_failure(0);
        let _ = unhealthy_pool.mark_failure(1);

        for strategy in [
            "round-robin",
            "consistent-hash",
            "random",
            "least-connections",
            "latency-aware",
            "sticky-cid",
        ] {
            let mut lb = LoadBalancing::from_config(strategy).expect("strategy");
            assert!(
                lb.pick("tenant:alpha", &unhealthy_pool).is_none(),
                "strategy {strategy} should not select from an all-unhealthy backend pool"
            );
        }
    }
}
