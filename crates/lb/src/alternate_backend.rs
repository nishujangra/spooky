//! Alternate-backend selection façade.
//!
//! This module owns retry/hedge follow-up backend choice on top of
//! [`UpstreamPool`]. It consumes pool-level read APIs and does not mutate pool
//! internals directly.

use crate::upstream_pool::UpstreamPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlternateBackendSelectionMode {
    LoadBalancerReadonly,
    HealthyFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AlternateBackendChoice {
    pub index: usize,
    pub mode: AlternateBackendSelectionMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlternateBackendFailureReason {
    NoHealthyBackends,
    OnlyExcludedBackendsHealthy,
    PoolUnavailable,
    BackendAddressMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlternateBackendDecision {
    Select(AlternateBackendChoice),
    DoNotSelect {
        denial: AlternateBackendFailureReason,
    },
}

fn is_excluded(index: usize, excluded_indices: &[usize]) -> bool {
    excluded_indices.contains(&index)
}

pub fn choose_alternate_backend(
    pool: &UpstreamPool,
    excluded_indices: &[usize],
    lb_key: Option<&str>,
) -> AlternateBackendDecision {
    let policy = pool.alternate_backend_policy();

    if policy.readonly_lb_pick {
        let readonly_candidate = pool
            .pick_readonly(lb_key.unwrap_or_default())
            .filter(|index| !is_excluded(*index, excluded_indices));
        if let Some(index) = readonly_candidate {
            return AlternateBackendDecision::Select(AlternateBackendChoice {
                index,
                mode: AlternateBackendSelectionMode::LoadBalancerReadonly,
            });
        }
    }

    if policy.healthy_fallback {
        let fallback_candidate = pool
            .healthy_backend_indices_iter()
            .find(|index| !is_excluded(*index, excluded_indices));
        if let Some(index) = fallback_candidate {
            return AlternateBackendDecision::Select(AlternateBackendChoice {
                index,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            });
        }
    }

    if pool.membership_summary().healthy_backends == 0 {
        AlternateBackendDecision::DoNotSelect {
            denial: AlternateBackendFailureReason::NoHealthyBackends,
        }
    } else {
        AlternateBackendDecision::DoNotSelect {
            denial: AlternateBackendFailureReason::OnlyExcludedBackendsHealthy,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use spooky_config::{
        config::{Backend, Config, HealthCheck, Listen, LoadBalancing, RouteMatch, Tls, Upstream},
        runtime::{RuntimeAlternateBackendPolicy, RuntimeConfig, RuntimeUpstream},
    };

    use super::*;
    fn upstream(lb_type: &str, backends: &[&str]) -> Upstream {
        Upstream {
            tls: None,
            load_balancing: LoadBalancing {
                lb_type: lb_type.to_string(),
                key: None,
            },
            auth: Default::default(),
            host_policy: Default::default(),
            forwarded_headers: Default::default(),
            route: RouteMatch::default(),
            backends: backends
                .iter()
                .enumerate()
                .map(|(index, address)| Backend {
                    id: format!("backend-{index}"),
                    address: (*address).to_string(),
                    weight: 1,
                    health_check: Some(HealthCheck {
                        path: "/health".to_string(),
                        interval: 1,
                        timeout_ms: 1000,
                        failure_threshold: 1,
                        success_threshold: 1,
                        cooldown_ms: 1000,
                    }),
                })
                .collect(),
        }
    }

    fn runtime_upstream(upstream: Upstream) -> RuntimeUpstream {
        let mut upstreams = HashMap::new();
        upstreams.insert("api".to_string(), upstream);

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
    fn chooses_non_excluded_backend_from_readonly_lb_pick() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b", "http://c"],
        )))
        .expect("pool");

        let decision = choose_alternate_backend(&pool, &[2], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 0,
                mode: AlternateBackendSelectionMode::LoadBalancerReadonly,
            })
        );
    }

    #[test]
    fn falls_back_when_readonly_pick_hits_excluded_backend() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b", "http://c"],
        )))
        .expect("pool");

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 1,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            })
        );
    }

    #[test]
    fn excludes_multiple_prior_backends_before_selecting_an_alternate() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b", "http://c", "http://d"],
        )))
        .expect("pool");

        let decision = choose_alternate_backend(&pool, &[0, 1, 2], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 3,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            })
        );
    }

    #[test]
    fn falls_back_to_healthy_scan_when_readonly_strategy_is_unavailable() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "consistent-hash",
            &["http://a", "http://b"],
        )))
        .expect("pool");

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 1,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            })
        );
    }

    #[test]
    fn ignores_unhealthy_backends_when_scanning_for_alternates() {
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b", "http://c"],
        )))
        .expect("pool");

        let _ = pool.mark_backend_failure_from_active_check(1);

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 2,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            })
        );
    }

    #[test]
    fn reports_when_only_excluded_backends_are_healthy() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a"],
        )))
        .expect("pool");

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::DoNotSelect {
                denial: AlternateBackendFailureReason::OnlyExcludedBackendsHealthy,
            }
        );
    }

    #[test]
    fn readonly_strategy_interaction_uses_load_balancer_mode_for_alternates() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b", "http://c"],
        )))
        .expect("pool");

        let first = choose_alternate_backend(&pool, &[], Some("tenant-a"));
        let second = choose_alternate_backend(&pool, &[], Some("tenant-b"));

        assert!(matches!(
            first,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                mode: AlternateBackendSelectionMode::LoadBalancerReadonly,
                ..
            })
        ));
        assert!(matches!(
            second,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                mode: AlternateBackendSelectionMode::LoadBalancerReadonly,
                ..
            })
        ));
    }

    #[test]
    fn non_readonly_strategies_use_healthy_fallback_for_alternates() {
        let pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "consistent-hash",
            &["http://a", "http://b", "http://c"],
        )))
        .expect("pool");

        let decision = choose_alternate_backend(&pool, &[0, 1], Some("tenant-a"));
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 2,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            })
        );
    }

    #[test]
    fn reports_when_no_backends_are_healthy() {
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b"],
        )))
        .expect("pool");

        let _ = pool.mark_backend_failure_from_active_check(0);
        let _ = pool.mark_backend_failure_from_active_check(1);

        let decision = choose_alternate_backend(&pool, &[], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::DoNotSelect {
                denial: AlternateBackendFailureReason::NoHealthyBackends,
            }
        );
    }

    #[test]
    fn denial_reason_stays_no_healthy_backends_even_when_failover_modes_are_disabled() {
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b"],
        )))
        .expect("pool");
        pool.set_alternate_backend_policy(RuntimeAlternateBackendPolicy {
            readonly_lb_pick: false,
            healthy_fallback: false,
        });
        let _ = pool.mark_backend_failure_from_active_check(0);
        let _ = pool.mark_backend_failure_from_active_check(1);

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::DoNotSelect {
                denial: AlternateBackendFailureReason::NoHealthyBackends,
            }
        );
    }

    #[test]
    fn suppresses_readonly_pick_when_policy_disables_it() {
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b", "http://c"],
        )))
        .expect("pool");
        pool.set_alternate_backend_policy(RuntimeAlternateBackendPolicy {
            readonly_lb_pick: false,
            healthy_fallback: true,
        });

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::Select(AlternateBackendChoice {
                index: 1,
                mode: AlternateBackendSelectionMode::HealthyFallback,
            })
        );
    }

    #[test]
    fn reports_excluded_backends_when_all_failover_modes_are_disabled() {
        let mut pool = UpstreamPool::from_runtime_upstream(&runtime_upstream(upstream(
            "round-robin",
            &["http://a", "http://b"],
        )))
        .expect("pool");
        pool.set_alternate_backend_policy(RuntimeAlternateBackendPolicy {
            readonly_lb_pick: false,
            healthy_fallback: false,
        });

        let decision = choose_alternate_backend(&pool, &[0], None);
        assert_eq!(
            decision,
            AlternateBackendDecision::DoNotSelect {
                denial: AlternateBackendFailureReason::OnlyExcludedBackendsHealthy,
            }
        );
    }
}
