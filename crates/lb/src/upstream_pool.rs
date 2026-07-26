//! Canonical upstream pool façade.
//!
//! [`UpstreamPool`] owns balancing primitives plus the narrow lifecycle-facing
//! mutation surface that edge/runtime code is allowed to use. Strategy state is
//! encapsulated here; callers should not reach into [`BackendPool`] directly.

use std::time::Duration;

use spooky_config::runtime::{
    RuntimeAlternateBackendPolicy, RuntimeLoadBalancingPolicy, RuntimeLoadBalancingStrategy,
    RuntimeRequestKeySpec, RuntimeUpstream,
};

use crate::{
    backend::BackendState,
    backend_pool::BackendPool,
    health::{HealthFailureReason, HealthTransition},
    load_balancing::LoadBalancing,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UpstreamPoolMembershipSummary {
    pub total_backends: usize,
    pub healthy_backends: usize,
    pub membership_epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpstreamBackendRuntimeState {
    pub healthy: bool,
    pub active_requests: usize,
    pub ewma_latency_ms: Option<f64>,
}

pub struct UpstreamPool {
    pool: BackendPool,
    load_balancer: LoadBalancing,
    lb_policy: RuntimeLoadBalancingPolicy,
}

impl UpstreamPool {
    pub fn from_runtime_upstream(upstream: &RuntimeUpstream) -> Result<Self, String> {
        let backends = upstream
            .backends
            .iter()
            .map(|backend| BackendState::new(&backend.backend))
            .collect();

        let lb_policy = upstream.load_balancing.clone();
        let load_balancer = LoadBalancing::from_runtime_strategy(lb_policy.strategy)?;

        Ok(Self {
            pool: BackendPool::new_from_states(backends),
            load_balancer,
            lb_policy,
        })
    }

    pub fn pick(&mut self, key: &str) -> Option<usize> {
        self.pool.reconcile_readmit();
        let selected = self.load_balancer.pick(key, &self.pool)?;
        self.pool.begin_request(selected);
        Some(selected)
    }

    pub fn pick_readonly(&self, key: &str) -> Option<usize> {
        self.load_balancer.pick_readonly(key, &self.pool)
    }

    pub fn pick_without_begin(&mut self, key: &str) -> Option<usize> {
        self.pool.reconcile_readmit();
        self.load_balancer.pick(key, &self.pool)
    }

    pub fn begin_request_if_healthy(&self, index: usize) -> bool {
        if self.pool.is_healthy_index(index) {
            self.pool.begin_request(index);
            true
        } else {
            false
        }
    }

    pub fn finish_request(&mut self, index: usize, latency: Duration, status: Option<u16>) {
        self.pool.finish_request(index, latency, status);
    }

    pub fn mark_backend_healthy(&mut self, index: usize) -> Option<HealthTransition> {
        self.pool.mark_success(index)
    }

    pub fn mark_backend_failure_from_active_check(
        &mut self,
        index: usize,
    ) -> Option<HealthTransition> {
        self.pool.mark_failure(index)
    }

    pub fn mark_backend_request_failure(
        &mut self,
        index: usize,
        reason: HealthFailureReason,
    ) -> Option<HealthTransition> {
        self.pool.mark_request_failure(index, reason)
    }

    pub fn backend_address(&self, index: usize) -> Option<&str> {
        self.pool.address(index)
    }

    pub fn backend_count(&self) -> usize {
        self.pool.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }

    pub fn backend_indices(&self) -> Vec<usize> {
        self.pool.all_indices()
    }

    pub fn is_backend_healthy(&self, index: usize) -> bool {
        self.pool.is_healthy_index(index)
    }

    pub fn begin_request_for_accounting(&self, index: usize) {
        self.pool.begin_request(index);
    }

    pub fn backend_runtime_state(&self, index: usize) -> Option<UpstreamBackendRuntimeState> {
        let backend = self.pool.backend(index)?;
        Some(UpstreamBackendRuntimeState {
            healthy: self.pool.is_healthy_index(index),
            active_requests: backend.active_requests(),
            ewma_latency_ms: backend.ewma_latency_ms(),
        })
    }

    pub fn healthy_backend_indices_iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.pool.healthy_indices_iter()
    }

    pub fn membership_summary(&self) -> UpstreamPoolMembershipSummary {
        UpstreamPoolMembershipSummary {
            total_backends: self.pool.len(),
            healthy_backends: self.pool.healthy_len(),
            membership_epoch: self.pool.membership_epoch(),
        }
    }

    pub fn lb_policy(&self) -> &RuntimeLoadBalancingPolicy {
        &self.lb_policy
    }

    pub fn lb_strategy(&self) -> RuntimeLoadBalancingStrategy {
        self.lb_policy.strategy
    }

    pub fn load_balancer_name(&self) -> &'static str {
        self.load_balancer.name()
    }

    pub fn lb_key_spec(&self) -> Option<&RuntimeRequestKeySpec> {
        self.lb_policy.key_spec.as_ref()
    }

    pub fn alternate_backend_policy(&self) -> RuntimeAlternateBackendPolicy {
        self.lb_policy.alternate_backend
    }

    #[cfg(test)]
    pub(crate) fn set_alternate_backend_policy(&mut self, policy: RuntimeAlternateBackendPolicy) {
        self.lb_policy.alternate_backend = policy;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use spooky_config::{
        config::{Backend, Config, HealthCheck, Listen, LoadBalancing, RouteMatch, Tls, Upstream},
        runtime::{RuntimeConfig, RuntimeLoadBalancingStrategy, RuntimeRequestKeySpec},
    };

    use super::UpstreamPool;

    fn runtime_upstream(
        lb_type: &str,
        key: Option<&str>,
    ) -> spooky_config::runtime::RuntimeUpstream {
        let mut upstreams = HashMap::new();
        upstreams.insert(
            "api".to_string(),
            Upstream {
                tls: None,
                load_balancing: LoadBalancing {
                    lb_type: lb_type.to_string(),
                    key: key.map(str::to_string),
                },
                auth: Default::default(),
                host_policy: Default::default(),
                forwarded_headers: Default::default(),
                route: RouteMatch::default(),
                backends: vec![
                    Backend {
                        id: "backend-a".to_string(),
                        address: "http://127.0.0.1:7001".to_string(),
                        weight: 1,
                        health_check: Some(HealthCheck {
                            path: "/health".to_string(),
                            interval: 1,
                            timeout_ms: 1000,
                            failure_threshold: 1,
                            success_threshold: 1,
                            cooldown_ms: 1000,
                        }),
                    },
                    Backend {
                        id: "backend-b".to_string(),
                        address: "http://127.0.0.1:7002".to_string(),
                        weight: 1,
                        health_check: Some(HealthCheck {
                            path: "/health".to_string(),
                            interval: 1,
                            timeout_ms: 1000,
                            failure_threshold: 1,
                            success_threshold: 1,
                            cooldown_ms: 1000,
                        }),
                    },
                ],
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

    #[test]
    fn pick_without_begin_preserves_active_request_count_but_pick_increments_it() {
        let runtime_upstream = runtime_upstream("round-robin", None);
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream).expect("pool");

        let idx = pool.pick_without_begin("key").expect("readonly pick");
        assert_eq!(
            pool.backend_runtime_state(idx)
                .expect("runtime state")
                .active_requests,
            0
        );

        let picked = pool.pick("key").expect("mutable pick");
        assert_eq!(
            pool.backend_runtime_state(picked)
                .expect("runtime state")
                .active_requests,
            1
        );
    }

    #[test]
    fn pool_exposes_lb_key_spec_and_alternate_policy_from_runtime() {
        let consistent = UpstreamPool::from_runtime_upstream(&runtime_upstream(
            "consistent-hash",
            Some("header:x-user-id"),
        ))
        .expect("consistent pool");
        assert_eq!(
            consistent.lb_strategy(),
            RuntimeLoadBalancingStrategy::ConsistentHash
        );
        assert!(matches!(
            consistent.lb_key_spec(),
            Some(RuntimeRequestKeySpec::Header(name)) if name == "x-user-id"
        ));
        assert!(!consistent.alternate_backend_policy().readonly_lb_pick);
        assert!(consistent.alternate_backend_policy().healthy_fallback);

        let round_robin = UpstreamPool::from_runtime_upstream(&runtime_upstream(
            "round-robin",
            Some("authority"),
        ))
        .expect("round robin pool");
        assert_eq!(
            round_robin.lb_strategy(),
            RuntimeLoadBalancingStrategy::RoundRobin
        );
        assert!(matches!(
            round_robin.lb_key_spec(),
            Some(RuntimeRequestKeySpec::Authority)
        ));
        assert!(round_robin.alternate_backend_policy().readonly_lb_pick);
        assert!(round_robin.alternate_backend_policy().healthy_fallback);
    }

    #[test]
    fn membership_summary_and_runtime_state_stay_coherent_after_health_updates() {
        let runtime_upstream = runtime_upstream("round-robin", None);
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream).expect("pool");

        let before = pool.membership_summary();
        assert_eq!(before.total_backends, 2);
        assert_eq!(before.healthy_backends, 2);

        let picked = pool.pick("client-a").expect("pick");
        let runtime = pool.backend_runtime_state(picked).expect("runtime state");
        assert_eq!(runtime.active_requests, 1);
        assert!(runtime.healthy);

        let unhealthy = pool
            .mark_backend_failure_from_active_check(picked)
            .expect("health transition");
        assert!(matches!(unhealthy, crate::HealthTransition::BecameUnhealthy));
        let after_failure = pool.membership_summary();
        assert_eq!(after_failure.total_backends, 2);
        assert_eq!(after_failure.healthy_backends, 1);

        pool.finish_request(picked, std::time::Duration::from_millis(25), Some(503));
        let runtime = pool.backend_runtime_state(picked).expect("runtime state");
        assert_eq!(runtime.active_requests, 0);
        assert!(!runtime.healthy);

        assert!(pool.mark_backend_healthy(picked).is_none());
        let after_early_success = pool.membership_summary();
        assert_eq!(after_early_success.total_backends, 2);
        assert_eq!(after_early_success.healthy_backends, 1);
    }
}
