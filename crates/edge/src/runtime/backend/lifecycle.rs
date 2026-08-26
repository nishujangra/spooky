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
mod tests {
    use crate::runtime::backend::lifecycle::dns::ClientRotationOutcome;
    use std::sync::Arc;
    use std::sync::RwLock;
    use std::{collections::HashMap, net::SocketAddr, time::Duration};

    use impulse_config::{
        config::{Backend, Config, HealthCheck, Listen, LoadBalancing, RouteMatch, Tls, Upstream},
        runtime::{RuntimeBackendTransportKind, RuntimeConfig},
    };
    use impulse_lb::upstream_pool::UpstreamPool;
    use impulse_transport::{SharedDnsResolver, UpstreamTransportPool};

    use super::*;
    use crate::runtime::backend::event::{BackendHealthObservationOutcome, BackendRequestFeedback};

    fn updated_client_rotation(app: &BackendDnsRefreshApplication) -> &ClientRotationOutcome {
        match app {
            BackendDnsRefreshApplication::Updated {
                client_rotation, ..
            } => client_rotation,
            other => panic!("expected Updated, got {other:?}"),
        }
    }

    fn test_upstream_pool_with_interval(interval: u64) -> Arc<RwLock<UpstreamPool>> {
        let mut upstreams = std::collections::HashMap::new();
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

    fn test_upstream_pool() -> Arc<RwLock<UpstreamPool>> {
        test_upstream_pool_with_interval(0)
    }

    fn test_active_health_upstream_pool() -> Arc<RwLock<UpstreamPool>> {
        test_upstream_pool_with_interval(1000)
    }

    fn test_transport_pool(backend_addr: &str) -> UpstreamTransportPool {
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

    mod refresh_application {
        use super::*;
        use crate::Metrics;
        use crate::runtime::backend::resolution::RuntimeBackendResolution;
        use crate::runtime::backend::store::RuntimeBackendResolutionStore;
        use std::time::SystemTime;

        #[test]
        fn refresh_classification_maps_every_variant_and_preserves_traffic_on_failure() {
            let updated = BackendDnsRefreshApplication::Updated {
                backend_addr: "http://backend:8080".to_string(),
                authority_host: "backend".to_string(),
                previous_addrs: vec![],
                current_addrs: vec!["10.0.0.1:8080".parse().expect("addr")],
                generation: 1,
                refreshed_at: SystemTime::UNIX_EPOCH,
                client_rotation: ClientRotationOutcome::Rotated,
            };
            assert_eq!(
                updated.classification(),
                BackendRefreshClassification::Refreshed
            );
            assert!(!updated.traffic_continues_on_existing());

            let unchanged = BackendDnsRefreshApplication::Unchanged {
                backend_addr: "http://backend:8080".to_string(),
                authority_host: "backend".to_string(),
                current_addrs: vec!["10.0.0.1:8080".parse().expect("addr")],
                generation: 1,
                refreshed_at: SystemTime::UNIX_EPOCH,
            };
            assert_eq!(
                unchanged.classification(),
                BackendRefreshClassification::Unchanged
            );
            assert!(unchanged.traffic_continues_on_existing());

            let empty = BackendDnsRefreshApplication::EmptyAnswerRetained {
                backend_addr: "http://backend:8080".to_string(),
                authority_host: "backend".to_string(),
                retained_addrs: vec!["10.0.0.1:8080".parse().expect("addr")],
            };
            assert_eq!(
                empty.classification(),
                BackendRefreshClassification::Rejected
            );
            assert!(empty.traffic_continues_on_existing());
            assert!(empty.classification().is_failure());

            let failed = BackendDnsRefreshApplication::LookupFailed {
                backend_addr: "http://backend:8080".to_string(),
                authority_host: "backend".to_string(),
                retained_addrs: vec!["10.0.0.1:8080".parse().expect("addr")],
                error: "nxdomain".to_string(),
            };
            assert_eq!(
                failed.classification(),
                BackendRefreshClassification::FailedActivePreserved
            );
            assert!(failed.traffic_continues_on_existing());
            assert!(failed.classification().is_failure());
            assert_eq!(failed.backend_addr(), "http://backend:8080");
            assert_eq!(failed.authority_host(), "backend");
        }

        #[test]
        fn rotation_failure_is_metered_and_does_not_hide_refresh_success() {
            let metrics = Metrics::default();
            let updated = BackendDnsRefreshApplication::Updated {
                backend_addr: "http://backend.internal:8080".to_string(),
                authority_host: "backend.internal".to_string(),
                previous_addrs: vec!["10.0.0.1:8080".parse().expect("addr")],
                current_addrs: vec!["10.0.0.2:8080".parse().expect("addr")],
                generation: 2,
                refreshed_at: SystemTime::UNIX_EPOCH,
                client_rotation: ClientRotationOutcome::Failed {
                    error: "pool busy".to_string(),
                },
            };

            observe_backend_dns_refresh(&metrics, &updated);

            assert_eq!(
                metrics
                    .backend_client_rotation_failures
                    .load(std::sync::atomic::Ordering::Relaxed),
                1,
                "rotation failure must be metered, not silently dropped"
            );
            assert_eq!(
                metrics
                    .backend_client_rotations
                    .load(std::sync::atomic::Ordering::Relaxed),
                0
            );
            assert_eq!(
                updated_client_rotation(&updated).failure(),
                Some("pool busy")
            );
        }

        #[test]
        fn hostname_refresh_updates_resolved_addrs_and_generation() {
            let backend_addr = "http://backend.internal:8080";
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    backend_addr.to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(Arc::clone(&store));
            let resolver = SharedDnsResolver::new();
            let transport_pool = test_transport_pool(backend_addr);
            let backend = coordinator.backend(backend_addr).expect("backend");
            let new_addrs = vec!["10.0.0.10:8080".parse::<SocketAddr>().expect("addr")];

            let outcome = coordinator.apply_refresh(
                &backend,
                Ok(new_addrs.clone()),
                &resolver,
                &transport_pool,
            );

            let BackendDnsRefreshApplication::Updated {
                current_addrs,
                generation,
                client_rotation,
                ..
            } = &outcome
            else {
                panic!("expected Updated outcome, got: {outcome:?}");
            };
            assert_eq!(current_addrs, &new_addrs);
            assert_eq!(*generation, 1);
            assert!(client_rotation.rotated());

            let snapshot = coordinator
                .snapshot_backend(backend_addr)
                .expect("snapshot");
            assert_eq!(snapshot.resolution.resolved_addrs, new_addrs);
            assert_eq!(snapshot.resolution.refresh_generation, 1);
            assert_eq!(
                resolver.cached_addrs("backend.internal"),
                Some(vec!["10.0.0.10:0".parse::<SocketAddr>().expect("addr")])
            );
        }

        #[test]
        fn unchanged_refresh_does_not_rotate_clients_unnecessarily() {
            let backend_addr = "http://backend.internal:8080";
            let initial_addrs = vec!["10.0.0.10:8080".parse::<SocketAddr>().expect("addr")];
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    backend_addr.to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            store
                .apply_resolution_refresh(
                    backend_addr,
                    initial_addrs.clone(),
                    std::time::SystemTime::UNIX_EPOCH,
                )
                .expect("seed refresh");
            let coordinator = BackendLifecycleCoordinator::new(Arc::clone(&store));
            let resolver = SharedDnsResolver::new();
            let transport_pool = test_transport_pool(backend_addr);
            let backend = coordinator.backend(backend_addr).expect("backend");

            let outcome = coordinator.apply_refresh(
                &backend,
                Ok(initial_addrs.clone()),
                &resolver,
                &transport_pool,
            );

            assert!(matches!(
                outcome,
                BackendDnsRefreshApplication::Unchanged {
                    current_addrs,
                    generation: 2,
                    ..
                } if current_addrs == initial_addrs
            ));
        }

        #[test]
        fn successive_refreshes_increment_generation_across_outcomes() {
            let backend_addr = "http://backend.internal:8080";
            let initial_addrs = vec!["10.0.0.10:8080".parse::<SocketAddr>().expect("addr")];
            let updated_addrs = vec!["10.0.0.11:8080".parse::<SocketAddr>().expect("addr")];
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    backend_addr.to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            store
                .apply_resolution_refresh(
                    backend_addr,
                    initial_addrs,
                    std::time::SystemTime::UNIX_EPOCH,
                )
                .expect("seed refresh");
            let coordinator = BackendLifecycleCoordinator::new(Arc::clone(&store));
            let resolver = SharedDnsResolver::new();
            let transport_pool = test_transport_pool(backend_addr);
            let backend = coordinator.backend(backend_addr).expect("backend");

            let refreshed = coordinator.apply_refresh(
                &backend,
                Ok(updated_addrs.clone()),
                &resolver,
                &transport_pool,
            );
            assert!(matches!(
                refreshed,
                BackendDnsRefreshApplication::Updated {
                    generation: 2,
                    current_addrs,
                    ..
                } if current_addrs == updated_addrs
            ));

            let backend = coordinator.backend(backend_addr).expect("backend");
            let unchanged = coordinator.apply_refresh(
                &backend,
                Ok(updated_addrs.clone()),
                &resolver,
                &transport_pool,
            );
            assert!(matches!(
                unchanged,
                BackendDnsRefreshApplication::Unchanged {
                    generation: 3,
                    current_addrs,
                    ..
                } if current_addrs == updated_addrs
            ));

            let snapshot = coordinator
                .snapshot_backend(backend_addr)
                .expect("snapshot");
            assert_eq!(snapshot.resolution.resolved_addrs, updated_addrs);
            assert_eq!(snapshot.resolution.refresh_generation, 3);
        }

        #[test]
        fn empty_dns_answer_retains_prior_addresses() {
            let backend_addr = "http://backend.internal:8080";
            let retained_addrs = vec!["10.0.0.10:8080".parse::<SocketAddr>().expect("addr")];
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    backend_addr.to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            store
                .apply_resolution_refresh(
                    backend_addr,
                    retained_addrs.clone(),
                    std::time::SystemTime::UNIX_EPOCH,
                )
                .expect("seed refresh");
            let coordinator = BackendLifecycleCoordinator::new(store);
            let resolver = SharedDnsResolver::new();
            let transport_pool = test_transport_pool(backend_addr);
            let backend = coordinator.backend(backend_addr).expect("backend");

            let outcome =
                coordinator.apply_refresh(&backend, Ok(Vec::new()), &resolver, &transport_pool);

            assert!(matches!(
                outcome,
                BackendDnsRefreshApplication::EmptyAnswerRetained {
                    retained_addrs: actual,
                    ..
                } if actual == retained_addrs
            ));
        }

        #[test]
        fn failed_refresh_preserves_existing_resolution_and_generation() {
            let backend_addr = "http://backend.internal:8080";
            let retained_addrs = vec!["10.0.0.10:8080".parse::<SocketAddr>().expect("addr")];
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    backend_addr.to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            store
                .apply_resolution_refresh(
                    backend_addr,
                    retained_addrs.clone(),
                    std::time::SystemTime::UNIX_EPOCH,
                )
                .expect("seed refresh");
            let coordinator = BackendLifecycleCoordinator::new(Arc::clone(&store));
            let resolver = SharedDnsResolver::new();
            let transport_pool = test_transport_pool(backend_addr);
            let backend = coordinator.backend(backend_addr).expect("backend");

            let outcome = coordinator.apply_refresh(
                &backend,
                Err("nxdomain".to_string()),
                &resolver,
                &transport_pool,
            );

            assert!(matches!(
                outcome,
                BackendDnsRefreshApplication::LookupFailed {
                    retained_addrs: ref actual,
                    ..
                } if *actual == retained_addrs
            ));
            let snapshot = coordinator
                .snapshot_backend(backend_addr)
                .expect("snapshot after failed refresh");
            assert_eq!(snapshot.resolution.resolved_addrs, retained_addrs);
            assert_eq!(snapshot.resolution.refresh_generation, 1);
            assert_eq!(
                outcome.classification(),
                BackendRefreshClassification::FailedActivePreserved
            );
        }
    }

    mod snapshot_inventory {
        use impulse_lb::HealthTransition;

        use super::*;
        use crate::runtime::backend::resolution::RuntimeBackendResolution;
        use crate::runtime::backend::state::BackendHealthState;
        use crate::runtime::backend::state::BackendIdentity;
        use crate::runtime::backend::state::BackendMembershipState;
        use crate::runtime::backend::store::RuntimeBackendResolutionStore;
        use std::time::SystemTime;

        #[test]
        fn lifecycle_state_seeds_from_resolution_with_unknown_health() {
            let resolution = RuntimeBackendResolution::hostname(
                "https://backend.internal:8443".to_string(),
                "backend.internal".to_string(),
                8443,
            );

            let state = RuntimeBackendLifecycleState::from(&resolution);

            assert_eq!(state.identity.backend_addr, "https://backend.internal:8443");
            assert_eq!(state.membership, BackendMembershipState::Active);
            assert_eq!(state.health, BackendHealthState::Unknown);
            assert!(state.resolution.is_hostname());
        }

        #[test]
        fn lifecycle_coordinator_exposes_backend_snapshots() {
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "https://backend.internal:8443".to_string(),
                    "backend.internal".to_string(),
                    8443,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(Arc::clone(&store));

            let snapshot = coordinator
                .snapshot_backend("https://backend.internal:8443")
                .expect("backend snapshot");
            assert_eq!(
                snapshot.identity.backend_addr,
                "https://backend.internal:8443"
            );

            let all = coordinator.snapshot_all();
            assert_eq!(all.len(), 1);
            assert!(all.contains_key("https://backend.internal:8443"));
        }

        #[test]
        fn backend_snapshot_exposes_resolved_addresses_and_refresh_generation() {
            let backend_addr = "https://backend.internal:8443";
            let resolved_addrs = vec![
                "10.0.0.10:8443".parse::<SocketAddr>().expect("addr"),
                "10.0.0.11:8443".parse::<SocketAddr>().expect("addr"),
            ];
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    backend_addr.to_string(),
                    "backend.internal".to_string(),
                    8443,
                ),
            ]));
            store
                .apply_resolution_refresh(
                    backend_addr,
                    resolved_addrs.clone(),
                    SystemTime::UNIX_EPOCH,
                )
                .expect("seed refresh");
            let coordinator = BackendLifecycleCoordinator::new(store);

            let snapshot = coordinator
                .snapshot_backend(backend_addr)
                .expect("backend snapshot");

            assert_eq!(snapshot.identity.backend_addr, backend_addr);
            assert_eq!(snapshot.resolution.authority_host, "backend.internal");
            assert_eq!(snapshot.resolution.authority_port, 8443);
            assert_eq!(snapshot.resolution.resolved_addrs, resolved_addrs);
            assert_eq!(snapshot.resolution.refresh_generation, 1);
            assert_eq!(
                snapshot.resolution.last_refresh_success_at,
                Some(SystemTime::UNIX_EPOCH)
            );
            assert!(matches!(snapshot.health, BackendHealthState::Unknown));
            assert_eq!(snapshot.membership, BackendMembershipState::Active);
        }

        #[test]
        fn lifecycle_coordinator_merges_resolution_and_pool_health_into_inventory() {
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);
            let mut pools = HashMap::new();
            pools.insert("api".to_string(), test_upstream_pool());

            let inventory = coordinator.snapshot_inventory(&pools);
            let backend = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:8080")
                .expect("backend inventory");

            assert_eq!(inventory.summary().healthy_backends, 1);
            assert_eq!(inventory.summary().total_backends, 1);
            assert_eq!(backend.placements.len(), 1);
            assert!(matches!(backend.health, BackendHealthState::Healthy));
            assert_eq!(backend.placements[0].upstream_name, "api");
        }

        #[test]
        fn inventory_marks_missing_pool_members_as_removed_without_losing_live_placements() {
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend-a.internal".to_string(),
                    8080,
                ),
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:9090".to_string(),
                    "backend-b.internal".to_string(),
                    9090,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);
            let mut pools = HashMap::new();
            pools.insert("api".to_string(), test_upstream_pool());

            let inventory = coordinator.snapshot_inventory(&pools);
            let active = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:8080")
                .expect("active backend");
            let removed = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:9090")
                .expect("removed backend");

            assert_eq!(active.membership, BackendMembershipState::Active);
            assert_eq!(active.placements.len(), 1);
            assert_eq!(removed.membership, BackendMembershipState::Removed);
            assert!(removed.placements.is_empty());
            assert_eq!(inventory.summary().total_backends, 1);
        }

        #[test]
        fn inventory_exposes_canonical_health_membership_and_resolution_views() {
            let placed_backend = "127.0.0.1:8080";
            let removed_backend = "127.0.0.1:9090";
            let resolved_addrs = vec!["10.0.0.10:8080".parse::<SocketAddr>().expect("addr")];
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    placed_backend.to_string(),
                    "backend-a.internal".to_string(),
                    8080,
                ),
                RuntimeBackendResolution::hostname(
                    removed_backend.to_string(),
                    "backend-b.internal".to_string(),
                    9090,
                ),
            ]));
            store
                .apply_resolution_refresh(
                    placed_backend,
                    resolved_addrs.clone(),
                    SystemTime::UNIX_EPOCH,
                )
                .expect("seed refresh");
            let coordinator = BackendLifecycleCoordinator::new(Arc::clone(&store));
            let pool = test_active_health_upstream_pool();
            let failure = evaluate_active_health_check(
                BackendIdentity::new(placed_backend),
                BackendHealthObservationOutcome::Failure,
                Some(impulse_lb::health::HealthFailureReason::Transport),
                100,
                0,
            );
            let transition =
                coordinator.apply_health_observation(Some(&pool), Some(0), &failure.observation);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let mut pools = HashMap::new();
            pools.insert("api".to_string(), pool);
            let inventory = coordinator.snapshot_inventory(&pools);

            let active = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == placed_backend)
                .expect("active backend");
            assert_eq!(active.identity.backend_addr, placed_backend);
            assert_eq!(active.membership, BackendMembershipState::Active);
            assert!(matches!(
                active.health,
                BackendHealthState::Unhealthy { reason: None }
            ));
            assert_eq!(active.resolution.resolved_addrs, resolved_addrs);
            assert_eq!(active.resolution.refresh_generation, 1);
            assert_eq!(
                active.resolution.last_refresh_success_at,
                Some(SystemTime::UNIX_EPOCH)
            );
            assert_eq!(active.placements.len(), 1);
            assert!(!active.placements[0].healthy);

            let removed = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == removed_backend)
                .expect("removed backend");
            assert_eq!(removed.identity.backend_addr, removed_backend);
            assert_eq!(removed.membership, BackendMembershipState::Removed);
            assert!(matches!(removed.health, BackendHealthState::Unknown));
            assert!(removed.placements.is_empty());

            assert_eq!(inventory.summary().total_backends, 1);
            assert_eq!(inventory.summary().healthy_backends, 0);
        }
    }

    mod health_observation_application {
        use impulse_lb::HealthTransition;

        use super::*;
        use crate::runtime::backend::event::BackendHealthObservation;
        use crate::runtime::backend::event::BackendHealthObservationSource;
        use crate::runtime::backend::resolution::RuntimeBackendResolution;
        use crate::runtime::backend::state::BackendHealthState;
        use crate::runtime::backend::state::BackendIdentity;
        use crate::runtime::backend::store::RuntimeBackendResolutionStore;

        #[test]
        fn active_health_check_evaluation_tracks_backoff_and_transition() {
            let pool = test_active_health_upstream_pool();

            let failure = evaluate_active_health_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Failure,
                Some(impulse_lb::health::HealthFailureReason::Transport),
                100,
                0,
            );
            assert_eq!(failure.next_consecutive_failures, 1);
            assert_eq!(failure.next_delay, Duration::from_millis(200));
            let transition =
                apply_backend_health_observation(Some(&pool), Some(0), &failure.observation);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let success = evaluate_active_health_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Success,
                None,
                100,
                failure.next_consecutive_failures,
            );
            assert_eq!(success.next_consecutive_failures, 0);
            assert_eq!(success.next_delay, Duration::from_millis(100));
            let transition =
                apply_backend_health_observation(Some(&pool), Some(0), &success.observation);
            assert!(matches!(transition, Some(HealthTransition::BecameHealthy)));
        }

        #[test]
        fn success_observation_keeps_healthy_backend_aligned_with_pool_state() {
            let pool = test_active_health_upstream_pool();
            let observation = BackendHealthObservation::active_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Success,
                None,
            );

            let transition = apply_backend_health_observation(Some(&pool), Some(0), &observation);
            assert!(
                transition.is_none(),
                "healthy backend should not re-transition"
            );

            let guard = pool.read().expect("read");
            let state = guard.backend_runtime_state(0).expect("backend state");
            assert!(state.healthy, "pool runtime state must remain healthy");
        }

        #[test]
        fn coordinator_health_observation_marks_backend_unhealthy_and_recovers() {
            let pool = test_active_health_upstream_pool();
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);

            let failure = evaluate_active_health_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Failure,
                Some(impulse_lb::health::HealthFailureReason::Transport),
                100,
                0,
            );
            let transition =
                coordinator.apply_health_observation(Some(&pool), Some(0), &failure.observation);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let mut pools = HashMap::new();
            pools.insert("api".to_string(), Arc::clone(&pool));
            let summary = coordinator.snapshot_inventory(&pools).summary();
            assert_eq!(summary.healthy_backends, 0);
            assert_eq!(summary.total_backends, 1);

            let success = evaluate_active_health_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Success,
                None,
                100,
                failure.next_consecutive_failures,
            );
            let transition =
                coordinator.apply_health_observation(Some(&pool), Some(0), &success.observation);
            assert!(matches!(transition, Some(HealthTransition::BecameHealthy)));

            let summary = coordinator.snapshot_inventory(&pools).summary();
            assert_eq!(summary.healthy_backends, 1);

            let backend = coordinator
                .snapshot_inventory(&pools)
                .backends
                .into_iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:8080")
                .expect("backend inventory");
            assert!(matches!(backend.health, BackendHealthState::Healthy));
            assert!(backend.placements[0].healthy);
        }

        #[test]
        fn passive_failure_observation_requires_reason_and_threshold_before_unhealthy() {
            let pool = test_upstream_pool();
            let no_reason = BackendHealthObservation {
                identity: BackendIdentity::new("127.0.0.1:8080"),
                source: BackendHealthObservationSource::PassiveRequest,
                outcome: BackendHealthObservationOutcome::Failure,
                reason: None,
            };

            let transition = apply_backend_health_observation(Some(&pool), Some(0), &no_reason);
            assert!(
                transition.is_none(),
                "missing reason must not mutate health"
            );
            assert!(
                pool.read().expect("read").is_backend_healthy(0),
                "pool should remain healthy without a mapped reason"
            );

            let failure = BackendHealthObservation {
                identity: BackendIdentity::new("127.0.0.1:8080"),
                source: BackendHealthObservationSource::PassiveRequest,
                outcome: BackendHealthObservationOutcome::Failure,
                reason: Some(impulse_lb::health::HealthFailureReason::Timeout),
            };

            assert!(apply_backend_health_observation(Some(&pool), Some(0), &failure).is_none());
            assert!(apply_backend_health_observation(Some(&pool), Some(0), &failure).is_none());
            let transition = apply_backend_health_observation(Some(&pool), Some(0), &failure);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let guard = pool.read().expect("read");
            let state = guard.backend_runtime_state(0).expect("backend state");
            assert!(
                !state.healthy,
                "pool runtime state must reflect passive failure threshold crossing"
            );
        }

        #[test]
        fn passive_failure_observation_keeps_inventory_aligned_with_pool_state() {
            let pool = test_upstream_pool();
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);
            let mut pools = HashMap::new();
            pools.insert("api".to_string(), Arc::clone(&pool));

            let failure = BackendHealthObservation {
                identity: BackendIdentity::new("127.0.0.1:8080"),
                source: BackendHealthObservationSource::PassiveRequest,
                outcome: BackendHealthObservationOutcome::Failure,
                reason: Some(impulse_lb::health::HealthFailureReason::Transport),
            };
            assert!(
                coordinator
                    .apply_health_observation(Some(&pool), Some(0), &failure)
                    .is_none()
            );
            assert!(
                coordinator
                    .apply_health_observation(Some(&pool), Some(0), &failure)
                    .is_none()
            );
            let transition = coordinator.apply_health_observation(Some(&pool), Some(0), &failure);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let inventory = coordinator.snapshot_inventory(&pools);
            let backend = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:8080")
                .expect("backend inventory after failure");
            assert!(matches!(
                backend.health,
                BackendHealthState::Unhealthy { reason: None }
            ));
            assert!(
                !backend.placements[0].healthy,
                "placement health must match lifecycle unhealthy state"
            );
        }

        #[test]
        fn passive_success_does_not_override_health_without_transition() {
            let pool = test_upstream_pool();
            let failure = BackendHealthObservation {
                identity: BackendIdentity::new("127.0.0.1:8080"),
                source: BackendHealthObservationSource::PassiveRequest,
                outcome: BackendHealthObservationOutcome::Failure,
                reason: Some(impulse_lb::health::HealthFailureReason::Transport),
            };
            assert!(apply_backend_health_observation(Some(&pool), Some(0), &failure).is_none());
            assert!(apply_backend_health_observation(Some(&pool), Some(0), &failure).is_none());
            assert!(matches!(
                apply_backend_health_observation(Some(&pool), Some(0), &failure),
                Some(HealthTransition::BecameUnhealthy)
            ));

            let success = BackendHealthObservation {
                identity: BackendIdentity::new("127.0.0.1:8080"),
                source: BackendHealthObservationSource::PassiveRequest,
                outcome: BackendHealthObservationOutcome::Success,
                reason: None,
            };
            let transition = apply_backend_health_observation(Some(&pool), Some(0), &success);
            assert!(
                transition.is_none(),
                "passive success should not invent a recovery transition on its own"
            );
            assert!(
                !pool.read().expect("read").is_backend_healthy(0),
                "pool should remain unhealthy until the proper recovery path runs"
            );
        }
    }

    mod request_feedback_application {
        use impulse_lb::HealthTransition;

        use super::*;
        use crate::runtime::backend::resolution::RuntimeBackendResolution;
        use crate::runtime::backend::state::BackendHealthState;
        use crate::runtime::backend::state::BackendIdentity;
        use crate::runtime::backend::store::RuntimeBackendResolutionStore;

        #[test]
        fn request_feedback_applier_marks_backend_unhealthy_after_failure_threshold() {
            let pool = test_upstream_pool();
            let feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(10),
                Some(503),
                Some(impulse_lb::health::HealthFailureReason::HttpStatus5xx),
            );
            assert!(apply_backend_request_feedback(Some(&pool), Some(0), &feedback).is_none());
            assert!(apply_backend_request_feedback(Some(&pool), Some(0), &feedback).is_none());
            let unhealthy = apply_backend_request_feedback(Some(&pool), Some(0), &feedback);
            assert!(matches!(unhealthy, Some(HealthTransition::BecameUnhealthy)));
        }

        #[test]
        fn request_feedback_success_keeps_backend_healthy_without_transition() {
            let pool = test_upstream_pool();
            let feedback = BackendRequestFeedback::from_status(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(10),
                http::StatusCode::OK,
            );

            let transition = apply_backend_request_feedback(Some(&pool), Some(0), &feedback);
            assert!(
                transition.is_none(),
                "healthy request completion should not invent a transition"
            );
            assert!(
                pool.read().expect("read").is_backend_healthy(0),
                "success feedback must preserve healthy state"
            );
        }

        #[test]
        fn request_feedback_client_error_is_neutral_for_health() {
            let pool = test_upstream_pool();
            let feedback = BackendRequestFeedback::from_status(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(10),
                http::StatusCode::NOT_FOUND,
            );

            let transition = apply_backend_request_feedback(Some(&pool), Some(0), &feedback);
            assert!(
                transition.is_none(),
                "client errors should not mutate backend health"
            );
            assert!(
                pool.read().expect("read").is_backend_healthy(0),
                "neutral feedback must leave pool health unchanged"
            );
        }

        #[test]
        fn request_feedback_timeout_and_transport_failures_share_health_effect_contract() {
            let timeout_pool = test_upstream_pool();
            let timeout_feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(100),
                None,
                Some(impulse_lb::health::HealthFailureReason::Timeout),
            );
            assert!(
                apply_backend_request_feedback(Some(&timeout_pool), Some(0), &timeout_feedback)
                    .is_none()
            );
            assert!(
                apply_backend_request_feedback(Some(&timeout_pool), Some(0), &timeout_feedback)
                    .is_none()
            );
            let timeout_transition =
                apply_backend_request_feedback(Some(&timeout_pool), Some(0), &timeout_feedback);
            assert!(matches!(
                timeout_transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let transport_pool = test_upstream_pool();
            let transport_feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(100),
                None,
                Some(impulse_lb::health::HealthFailureReason::Transport),
            );
            assert!(
                apply_backend_request_feedback(Some(&transport_pool), Some(0), &transport_feedback)
                    .is_none()
            );
            assert!(
                apply_backend_request_feedback(Some(&transport_pool), Some(0), &transport_feedback)
                    .is_none()
            );
            let transport_transition =
                apply_backend_request_feedback(Some(&transport_pool), Some(0), &transport_feedback);
            assert!(matches!(
                transport_transition,
                Some(HealthTransition::BecameUnhealthy)
            ));
        }

        #[test]
        fn request_feedback_without_reason_does_not_change_health() {
            let pool = test_upstream_pool();
            let feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(25),
                Some(503),
                None,
            );

            let transition = apply_backend_request_feedback(Some(&pool), Some(0), &feedback);
            assert!(
                transition.is_none(),
                "failure feedback without a mapped health reason must not mutate health"
            );
            assert!(
                pool.read().expect("read").is_backend_healthy(0),
                "reasonless failure feedback should stay a no-op for health"
            );
            let runtime = pool
                .read()
                .expect("read")
                .backend_runtime_state(0)
                .expect("backend state");
            assert!(
                runtime
                    .ewma_latency_ms
                    .is_some_and(|latency| latency >= 1_000.0)
            );
        }

        #[test]
        fn request_accounting_applier_finishes_inflight_request() {
            let pool = test_upstream_pool();
            {
                let guard = pool.read().expect("read");
                assert!(guard.begin_request_if_healthy(0));
            }

            apply_backend_request_accounting(
                Some(&pool),
                Some(0),
                Duration::from_millis(15),
                Some(200),
            );

            let guard = pool.read().expect("read");
            let state = guard.backend_runtime_state(0).expect("backend state");
            assert_eq!(state.active_requests, 0);
            assert!(state.ewma_latency_ms.is_some());
        }

        #[test]
        fn request_feedback_failure_penalizes_latency_even_with_active_health_checks() {
            let pool = test_active_health_upstream_pool();
            let feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(10),
                Some(503),
                Some(impulse_lb::health::HealthFailureReason::HttpStatus5xx),
            );

            let transition = apply_backend_request_feedback(Some(&pool), Some(0), &feedback);

            assert!(transition.is_none());
            let runtime = pool
                .read()
                .expect("read")
                .backend_runtime_state(0)
                .expect("backend state");
            assert!(runtime.healthy);
            assert!(
                runtime
                    .ewma_latency_ms
                    .is_some_and(|latency| latency >= 1_000.0)
            );
        }

        #[test]
        fn request_feedback_coordinator_inventory_tracks_timeout_failures() {
            let pool = test_upstream_pool();
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);
            let mut pools = HashMap::new();
            pools.insert("api".to_string(), Arc::clone(&pool));

            let feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(10),
                None,
                Some(impulse_lb::health::HealthFailureReason::Timeout),
            );
            assert!(apply_backend_request_feedback(Some(&pool), Some(0), &feedback).is_none());
            assert!(apply_backend_request_feedback(Some(&pool), Some(0), &feedback).is_none());
            let transition = apply_backend_request_feedback(Some(&pool), Some(0), &feedback);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let inventory = coordinator.snapshot_inventory(&pools);
            let backend = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:8080")
                .expect("backend inventory");
            assert!(matches!(
                backend.health,
                BackendHealthState::Unhealthy { reason: None }
            ));
            assert!(
                !backend.placements[0].healthy,
                "coordinator inventory must reflect the pool mutation contract instead of caller-local branching"
            );
        }

        #[test]
        fn coordinator_request_feedback_updates_inventory_consistently() {
            let pool = test_upstream_pool();
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);
            let mut pools = HashMap::new();
            pools.insert("api".to_string(), Arc::clone(&pool));

            let transition = apply_backend_request_feedback(
                Some(&pool),
                Some(0),
                &BackendRequestFeedback::failure(
                    BackendIdentity::new("127.0.0.1:8080"),
                    Duration::from_millis(10),
                    Some(503),
                    Some(impulse_lb::health::HealthFailureReason::HttpStatus5xx),
                ),
            );
            assert!(transition.is_none());

            let transition = apply_backend_request_feedback(
                Some(&pool),
                Some(0),
                &BackendRequestFeedback::failure(
                    BackendIdentity::new("127.0.0.1:8080"),
                    Duration::from_millis(10),
                    Some(503),
                    Some(impulse_lb::health::HealthFailureReason::HttpStatus5xx),
                ),
            );
            assert!(transition.is_none());

            let transition = apply_backend_request_feedback(
                Some(&pool),
                Some(0),
                &BackendRequestFeedback::failure(
                    BackendIdentity::new("127.0.0.1:8080"),
                    Duration::from_millis(10),
                    Some(503),
                    Some(impulse_lb::health::HealthFailureReason::HttpStatus5xx),
                ),
            );
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));

            let inventory = coordinator.snapshot_inventory(&pools);
            let backend = inventory
                .backends
                .iter()
                .find(|backend| backend.identity.backend_addr == "127.0.0.1:8080")
                .expect("backend inventory");
            assert!(matches!(
                backend.health,
                BackendHealthState::Unhealthy { .. }
            ));
            assert!(!backend.placements[0].healthy);
            assert_eq!(inventory.summary().healthy_backends, 0);
        }

        #[test]
        fn request_feedback_does_not_duplicate_active_health_check_ownership() {
            let pool = test_active_health_upstream_pool();
            let store = Arc::new(RuntimeBackendResolutionStore::new([
                RuntimeBackendResolution::hostname(
                    "127.0.0.1:8080".to_string(),
                    "backend.internal".to_string(),
                    8080,
                ),
            ]));
            let coordinator = BackendLifecycleCoordinator::new(store);
            let mut pools = HashMap::new();
            pools.insert("api".to_string(), Arc::clone(&pool));

            let transition = apply_backend_request_feedback(
                Some(&pool),
                Some(0),
                &BackendRequestFeedback::failure(
                    BackendIdentity::new("127.0.0.1:8080"),
                    Duration::from_millis(10),
                    Some(503),
                    Some(impulse_lb::health::HealthFailureReason::HttpStatus5xx),
                ),
            );
            assert!(transition.is_none());
            assert_eq!(
                coordinator
                    .snapshot_inventory(&pools)
                    .summary()
                    .healthy_backends,
                1
            );

            let failure = evaluate_active_health_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Failure,
                Some(impulse_lb::health::HealthFailureReason::Transport),
                100,
                0,
            );
            let transition =
                coordinator.apply_health_observation(Some(&pool), Some(0), &failure.observation);
            assert!(matches!(
                transition,
                Some(HealthTransition::BecameUnhealthy)
            ));
            assert_eq!(
                coordinator
                    .snapshot_inventory(&pools)
                    .summary()
                    .healthy_backends,
                0
            );
        }
    }
}
