use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime},
};

use log::{debug, info, warn};
use spooky_lb::{HealthTransition, upstream_pool::UpstreamPool};
use spooky_transport::{SharedDnsResolver, UpstreamTransportPool};

use super::{
    event::{
        BackendHealthObservation, BackendHealthObservationOutcome, BackendHealthObservationSource,
        BackendLifecycleMutation, BackendRefreshOutcome, BackendRequestFeedback,
        BackendRequestFeedbackOutcome,
    },
    resolution::RuntimeBackendResolution,
    state::{
        BackendHealthState, BackendIdentity, BackendLifecycleInventorySnapshot,
        BackendLifecycleSnapshot, BackendMembershipState, BackendPoolPlacementSnapshot,
        BackendResolutionState, CanonicalBackendLifecycleSnapshot,
    },
    store::RuntimeBackendResolutionStore,
};
use crate::Metrics;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendLifecycleState {
    pub identity: BackendIdentity,
    pub resolution: BackendResolutionState,
    pub health: BackendHealthState,
    pub membership: BackendMembershipState,
}

impl RuntimeBackendLifecycleState {
    pub fn new(
        identity: BackendIdentity,
        resolution: BackendResolutionState,
        health: BackendHealthState,
        membership: BackendMembershipState,
    ) -> Self {
        Self {
            identity,
            resolution,
            health,
            membership,
        }
    }

    pub fn from_resolution_seed(resolution: &RuntimeBackendResolution) -> Self {
        Self {
            identity: BackendIdentity::from(resolution),
            resolution: BackendResolutionState::from(resolution),
            health: BackendHealthState::Unknown,
            membership: BackendMembershipState::Active,
        }
    }

    pub fn snapshot(&self) -> BackendLifecycleSnapshot {
        BackendLifecycleSnapshot {
            identity: self.identity.clone(),
            resolution: self.resolution.clone(),
            health: self.health.clone(),
            membership: self.membership,
        }
    }
}

impl From<&RuntimeBackendResolution> for RuntimeBackendLifecycleState {
    fn from(value: &RuntimeBackendResolution) -> Self {
        Self::from_resolution_seed(value)
    }
}

impl From<&RuntimeBackendLifecycleState> for BackendLifecycleSnapshot {
    fn from(value: &RuntimeBackendLifecycleState) -> Self {
        value.snapshot()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveHealthCheckEvaluation {
    pub observation: BackendHealthObservation,
    pub next_consecutive_failures: u32,
    pub next_delay: Duration,
}

/// Explicit outcome of rotating pooled transport clients after a backend's
/// resolved addresses changed.
///
/// Phase 4: previously a bare `bool` collapsed a rotation *failure* into
/// "not rotated", hiding it from operators. Rotation failure is now a distinct,
/// operator-visible terminal state — the DNS refresh still succeeds (the new
/// addresses are recorded), but stale pooled connections may linger, so the
/// failure is logged and counted rather than silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClientRotationOutcome {
    /// Pooled clients were rotated to the new resolution.
    Rotated,
    /// No rotation was needed (transport reported the client already current).
    NotRotated,
    /// Rotation was attempted but failed; the DNS resolution update still stands.
    Failed { error: String },
}

impl ClientRotationOutcome {
    /// Whether pooled clients were actually rotated.
    #[cfg(test)]
    pub(crate) fn rotated(&self) -> bool {
        matches!(self, Self::Rotated)
    }

    /// The failure reason, if rotation failed.
    pub(crate) fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed { error } => Some(error.as_str()),
            Self::Rotated | Self::NotRotated => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendDnsRefreshApplication {
    Updated {
        backend_addr: String,
        authority_host: String,
        previous_addrs: Vec<SocketAddr>,
        current_addrs: Vec<SocketAddr>,
        generation: u64,
        refreshed_at: SystemTime,
        client_rotation: ClientRotationOutcome,
    },
    Unchanged {
        backend_addr: String,
        authority_host: String,
        current_addrs: Vec<SocketAddr>,
        generation: u64,
        refreshed_at: SystemTime,
    },
    EmptyAnswerRetained {
        backend_addr: String,
        authority_host: String,
        retained_addrs: Vec<SocketAddr>,
    },
    LookupFailed {
        backend_addr: String,
        authority_host: String,
        retained_addrs: Vec<SocketAddr>,
        error: String,
    },
}

/// The unified, operator-facing classification of a backend refresh outcome
/// (Phase 7).
///
/// Every internal [`BackendDnsRefreshApplication`] variant maps onto exactly one
/// of these so operators, logs, and metrics read a single result model instead of
/// recomputing intent from the specific variant. It also makes the safety
/// invariant explicit: no classification ever leaves partial hidden state — a
/// failure always preserves the existing resolution and keeps traffic on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendRefreshClassification {
    /// The backend's resolved addresses changed and were applied.
    Refreshed,
    /// The refresh ran but the resolved addresses were unchanged.
    Unchanged,
    /// The refresh produced no usable answer (empty result); the previous
    /// resolution is retained and serving.
    Rejected,
    /// The refresh failed; the active generation's resolution is preserved and
    /// still serving traffic.
    FailedActivePreserved,
}

impl BackendRefreshClassification {
    /// Whether traffic continues on the existing (pre-refresh) resolution.
    /// True for every non-`Refreshed` outcome — the whole point of the model is
    /// that a failed or empty refresh never drops the backend.
    pub fn traffic_continues_on_existing(self) -> bool {
        !matches!(self, Self::Refreshed)
    }

    /// Whether this classification represents a failure operators should act on.
    pub fn is_failure(self) -> bool {
        matches!(self, Self::Rejected | Self::FailedActivePreserved)
    }

    /// A stable, machine-readable slug for metric labels and structured logs.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Refreshed => "refreshed",
            Self::Unchanged => "unchanged",
            Self::Rejected => "rejected_empty_answer",
            Self::FailedActivePreserved => "failed_active_preserved",
        }
    }
}

impl BackendDnsRefreshApplication {
    /// The backend identity (canonical address) this outcome concerns.
    pub(crate) fn backend_addr(&self) -> &str {
        match self {
            Self::Updated { backend_addr, .. }
            | Self::Unchanged { backend_addr, .. }
            | Self::EmptyAnswerRetained { backend_addr, .. }
            | Self::LookupFailed { backend_addr, .. } => backend_addr,
        }
    }

    /// The authority host (upstream hostname) this outcome concerns.
    pub(crate) fn authority_host(&self) -> &str {
        match self {
            Self::Updated { authority_host, .. }
            | Self::Unchanged { authority_host, .. }
            | Self::EmptyAnswerRetained { authority_host, .. }
            | Self::LookupFailed { authority_host, .. } => authority_host,
        }
    }

    /// The unified operator-facing classification of this outcome.
    pub(crate) fn classification(&self) -> BackendRefreshClassification {
        match self {
            Self::Updated { .. } => BackendRefreshClassification::Refreshed,
            Self::Unchanged { .. } => BackendRefreshClassification::Unchanged,
            Self::EmptyAnswerRetained { .. } => BackendRefreshClassification::Rejected,
            Self::LookupFailed { .. } => BackendRefreshClassification::FailedActivePreserved,
        }
    }

    /// Whether traffic continues on the existing resolution after this outcome.
    #[cfg(test)]
    pub(crate) fn traffic_continues_on_existing(&self) -> bool {
        self.classification().traffic_continues_on_existing()
    }
}

#[derive(Debug, Clone)]
pub struct BackendLifecycleCoordinator {
    resolution_store: Arc<RuntimeBackendResolutionStore>,
}

impl BackendLifecycleCoordinator {
    pub fn new(resolution_store: Arc<RuntimeBackendResolutionStore>) -> Self {
        Self { resolution_store }
    }

    pub fn backend(&self, backend_addr: &str) -> Option<RuntimeBackendLifecycleState> {
        self.resolution_store.backend(backend_addr)
    }

    pub fn hostname_backends(&self) -> Vec<RuntimeBackendLifecycleState> {
        self.resolution_store.hostname_backends()
    }

    pub fn snapshot_backend(&self, backend_addr: &str) -> Option<BackendLifecycleSnapshot> {
        self.backend(backend_addr).map(|backend| backend.snapshot())
    }

    pub fn snapshot_all(&self) -> HashMap<String, BackendLifecycleSnapshot> {
        self.resolution_store.snapshot()
    }

    pub fn snapshot_inventory(
        &self,
        upstream_pools: &HashMap<String, Arc<RwLock<UpstreamPool>>>,
    ) -> BackendLifecycleInventorySnapshot {
        let mut snapshots = self
            .snapshot_all()
            .into_values()
            .map(|snapshot| {
                (
                    snapshot.identity.backend_addr.clone(),
                    CanonicalBackendLifecycleSnapshot {
                        identity: snapshot.identity,
                        resolution: snapshot.resolution,
                        health: snapshot.health,
                        membership: snapshot.membership,
                        placements: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        for (upstream_name, pool) in upstream_pools {
            let Ok(guard) = pool.read() else {
                continue;
            };
            let membership_summary = guard.membership_summary();
            for backend_index in guard.backend_indices() {
                let Some(backend_addr) = guard.backend_address(backend_index) else {
                    continue;
                };
                let Some(backend) = guard.backend_runtime_state(backend_index) else {
                    continue;
                };
                let entry = snapshots
                    .entry(backend_addr.to_string())
                    .or_insert_with(|| CanonicalBackendLifecycleSnapshot {
                        identity: BackendIdentity::new(backend_addr.to_string()),
                        resolution: BackendResolutionState {
                            authority_host: backend_addr.to_string(),
                            authority_port: 0,
                            address_kind: super::resolution::RuntimeBackendAddressKind::IpLiteral,
                            resolved_addrs: Vec::new(),
                            last_refresh_success_at: None,
                            refresh_generation: 0,
                        },
                        health: BackendHealthState::Unknown,
                        membership: BackendMembershipState::Removed,
                        placements: Vec::new(),
                    });
                entry.placements.push(BackendPoolPlacementSnapshot {
                    upstream_name: upstream_name.clone(),
                    backend_index,
                    healthy: backend.healthy,
                    active_requests: backend.active_requests,
                    ewma_latency_ms: backend.ewma_latency_ms,
                    membership_epoch: membership_summary.membership_epoch,
                });
            }
        }

        let mut backends = snapshots.into_values().collect::<Vec<_>>();
        for backend in &mut backends {
            if backend.placements.is_empty() {
                backend.membership = BackendMembershipState::Removed;
                continue;
            }

            backend.membership = BackendMembershipState::Active;
            backend.health = if backend.placements.iter().all(|placement| placement.healthy) {
                BackendHealthState::Healthy
            } else {
                BackendHealthState::Unhealthy { reason: None }
            };
            backend.placements.sort_by(|left, right| {
                left.upstream_name
                    .cmp(&right.upstream_name)
                    .then(left.backend_index.cmp(&right.backend_index))
            });
        }
        backends
            .sort_by(|left, right| left.identity.backend_addr.cmp(&right.identity.backend_addr));

        BackendLifecycleInventorySnapshot { backends }
    }

    pub(crate) fn apply_refresh(
        &self,
        backend: &RuntimeBackendLifecycleState,
        resolved_addrs: Result<Vec<SocketAddr>, String>,
        backend_dns_resolver: &SharedDnsResolver,
        transport_pool: &UpstreamTransportPool,
    ) -> BackendDnsRefreshApplication {
        apply_backend_dns_refresh(
            backend,
            resolved_addrs,
            self.resolution_store.as_ref(),
            backend_dns_resolver,
            transport_pool,
        )
    }

    pub(crate) fn apply_health_observation(
        &self,
        upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
        backend_index: Option<usize>,
        observation: &BackendHealthObservation,
    ) -> Option<HealthTransition> {
        apply_backend_health_observation(upstream_pool, backend_index, observation)
    }
}

pub(crate) fn apply_backend_request_accounting(
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    elapsed: Duration,
    status: Option<u16>,
) {
    if let (Some(pool), Some(index)) = (upstream_pool, backend_index)
        && let Ok(mut guard) = pool.write()
    {
        guard.finish_request(index, elapsed, status);
    }
}

pub(crate) fn apply_backend_request_feedback(
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    feedback: &BackendRequestFeedback,
) -> Option<HealthTransition> {
    let (Some(pool), Some(index)) = (upstream_pool, backend_index) else {
        return None;
    };
    let mut pool = pool.write().ok()?;
    match feedback.outcome {
        BackendRequestFeedbackOutcome::Success => pool.mark_backend_healthy(index),
        BackendRequestFeedbackOutcome::Neutral => None,
        BackendRequestFeedbackOutcome::Failure { reason } => {
            reason.and_then(|reason| pool.mark_backend_request_failure(index, reason))
        }
    }
}

pub(crate) fn evaluate_active_health_check(
    identity: BackendIdentity,
    outcome: BackendHealthObservationOutcome,
    reason: Option<spooky_lb::health::HealthFailureReason>,
    base_interval_ms: u64,
    consecutive_failures: u32,
) -> ActiveHealthCheckEvaluation {
    let next_consecutive_failures = match outcome {
        BackendHealthObservationOutcome::Failure => consecutive_failures.saturating_add(1),
        BackendHealthObservationOutcome::Success | BackendHealthObservationOutcome::Neutral => 0,
    };
    let backoff_multiplier = 1u64 << next_consecutive_failures.min(2);
    let delay_ms = base_interval_ms.saturating_mul(backoff_multiplier);

    ActiveHealthCheckEvaluation {
        observation: BackendHealthObservation::active_check(identity, outcome, reason),
        next_consecutive_failures,
        next_delay: Duration::from_millis(delay_ms),
    }
}

pub(crate) fn apply_backend_health_observation(
    upstream_pool: Option<&Arc<RwLock<UpstreamPool>>>,
    backend_index: Option<usize>,
    observation: &BackendHealthObservation,
) -> Option<HealthTransition> {
    let (Some(pool), Some(index)) = (upstream_pool, backend_index) else {
        return None;
    };
    let mut pool = pool.write().ok()?;
    match (observation.source, observation.outcome) {
        (BackendHealthObservationSource::ActiveCheck, BackendHealthObservationOutcome::Success) => {
            pool.mark_backend_healthy(index)
        }
        (BackendHealthObservationSource::ActiveCheck, BackendHealthObservationOutcome::Failure) => {
            pool.mark_backend_failure_from_active_check(index)
        }
        (BackendHealthObservationSource::ActiveCheck, BackendHealthObservationOutcome::Neutral) => {
            None
        }
        (_, BackendHealthObservationOutcome::Success) => pool.mark_backend_healthy(index),
        (_, BackendHealthObservationOutcome::Neutral) => None,
        (_, BackendHealthObservationOutcome::Failure) => observation
            .reason
            .and_then(|reason| pool.mark_backend_request_failure(index, reason)),
    }
}

pub(crate) fn apply_backend_dns_refresh(
    backend: &RuntimeBackendLifecycleState,
    resolved_addrs: Result<Vec<SocketAddr>, String>,
    resolution_store: &RuntimeBackendResolutionStore,
    backend_dns_resolver: &SharedDnsResolver,
    transport_pool: &UpstreamTransportPool,
) -> BackendDnsRefreshApplication {
    match resolved_addrs {
        Err(error) => BackendDnsRefreshApplication::LookupFailed {
            backend_addr: backend.identity.backend_addr.clone(),
            authority_host: backend.resolution.authority_host.clone(),
            retained_addrs: backend.resolution.resolved_addrs.clone(),
            error,
        },
        Ok(resolved) if resolved.is_empty() => BackendDnsRefreshApplication::EmptyAnswerRetained {
            backend_addr: backend.identity.backend_addr.clone(),
            authority_host: backend.resolution.authority_host.clone(),
            retained_addrs: backend.resolution.resolved_addrs.clone(),
        },
        Ok(resolved) => {
            let refreshed_at = SystemTime::now();
            let Some(mutation) = resolution_store.apply_resolution_refresh(
                &backend.identity.backend_addr,
                resolved.clone(),
                refreshed_at,
            ) else {
                return BackendDnsRefreshApplication::LookupFailed {
                    backend_addr: backend.identity.backend_addr.clone(),
                    authority_host: backend.resolution.authority_host.clone(),
                    retained_addrs: backend.resolution.resolved_addrs.clone(),
                    error: "hostname backend disappeared from resolution store".to_string(),
                };
            };

            backend_dns_resolver.set_host_addrs(
                &backend.resolution.authority_host,
                resolved
                    .into_iter()
                    .map(|addr| SocketAddr::new(addr.ip(), 0)),
            );

            let BackendLifecycleMutation::ResolutionUpdated { result, .. } = mutation else {
                return BackendDnsRefreshApplication::LookupFailed {
                    backend_addr: backend.identity.backend_addr.clone(),
                    authority_host: backend.resolution.authority_host.clone(),
                    retained_addrs: backend.resolution.resolved_addrs.clone(),
                    error: "unexpected backend lifecycle mutation during dns refresh".to_string(),
                };
            };

            let client_rotation = if matches!(result.outcome, BackendRefreshOutcome::Updated { .. })
            {
                // Phase 4: no longer collapse a rotation error into "not rotated".
                // A failure is preserved as an explicit outcome so it can be logged
                // and metered downstream.
                match transport_pool.rotate_backend_client(&result.identity.backend_addr) {
                    Ok(rotation) if rotation.rotated() => ClientRotationOutcome::Rotated,
                    Ok(_) => ClientRotationOutcome::NotRotated,
                    Err(error) => ClientRotationOutcome::Failed { error },
                }
            } else {
                ClientRotationOutcome::NotRotated
            };

            match result.outcome {
                BackendRefreshOutcome::Updated {
                    previous_addrs,
                    current_addrs,
                    refreshed_at,
                    refresh_generation,
                } => BackendDnsRefreshApplication::Updated {
                    backend_addr: result.identity.backend_addr,
                    authority_host: backend.resolution.authority_host.clone(),
                    previous_addrs,
                    current_addrs,
                    generation: refresh_generation,
                    refreshed_at: refreshed_at.unwrap_or_else(SystemTime::now),
                    client_rotation,
                },
                BackendRefreshOutcome::Unchanged {
                    current_addrs,
                    refreshed_at,
                    refresh_generation,
                } => BackendDnsRefreshApplication::Unchanged {
                    backend_addr: result.identity.backend_addr,
                    authority_host: backend.resolution.authority_host.clone(),
                    current_addrs,
                    generation: refresh_generation,
                    refreshed_at: refreshed_at.unwrap_or_else(SystemTime::now),
                },
                BackendRefreshOutcome::EmptyAnswerRetained { retained_addrs } => {
                    BackendDnsRefreshApplication::EmptyAnswerRetained {
                        backend_addr: result.identity.backend_addr,
                        authority_host: backend.resolution.authority_host.clone(),
                        retained_addrs,
                    }
                }
                BackendRefreshOutcome::LookupFailed {
                    retained_addrs,
                    error,
                } => BackendDnsRefreshApplication::LookupFailed {
                    backend_addr: result.identity.backend_addr,
                    authority_host: backend.resolution.authority_host.clone(),
                    retained_addrs,
                    error,
                },
            }
        }
    }
}

pub(crate) fn observe_backend_dns_refresh(
    metrics: &Metrics,
    outcome: &BackendDnsRefreshApplication,
) {
    // Phase 7: emit the unified classification so operators can read one result
    // model, and record whether traffic still flows on the existing resolution.
    let classification = outcome.classification();
    if classification.is_failure() {
        debug!(
            "backend refresh classification for '{}' (backend '{}'): {} traffic_continues_on_existing={}",
            outcome.authority_host(),
            outcome.backend_addr(),
            classification.slug(),
            classification.traffic_continues_on_existing()
        );
    }

    match outcome {
        BackendDnsRefreshApplication::Updated {
            backend_addr,
            current_addrs,
            refreshed_at,
            client_rotation,
            ..
        } => {
            metrics.record_backend_dns_refresh_success(
                backend_addr,
                *refreshed_at,
                current_addrs.len(),
                true,
            );
            match client_rotation {
                ClientRotationOutcome::Rotated => {
                    metrics.inc_backend_client_rotation(backend_addr);
                }
                ClientRotationOutcome::Failed { .. } => {
                    metrics.inc_backend_client_rotation_failure();
                }
                ClientRotationOutcome::NotRotated => {}
            }
        }
        BackendDnsRefreshApplication::Unchanged {
            backend_addr,
            current_addrs,
            refreshed_at,
            ..
        } => {
            metrics.record_backend_dns_refresh_success(
                backend_addr,
                *refreshed_at,
                current_addrs.len(),
                false,
            );
        }
        BackendDnsRefreshApplication::EmptyAnswerRetained { .. }
        | BackendDnsRefreshApplication::LookupFailed { .. } => {
            metrics.inc_backend_dns_refresh_failure();
        }
    }
}

pub(crate) fn log_backend_dns_refresh(outcome: &BackendDnsRefreshApplication) {
    match outcome {
        BackendDnsRefreshApplication::Updated {
            backend_addr,
            authority_host,
            previous_addrs,
            current_addrs,
            generation,
            client_rotation,
            ..
        } => {
            if previous_addrs.is_empty() {
                info!(
                    "backend DNS refresh populated '{}' (backend '{}') with {:?} generation={}",
                    authority_host, backend_addr, current_addrs, generation
                );
            } else {
                info!(
                    "backend DNS refresh updated '{}' (backend '{}'): {:?} -> {:?} generation={} stale_pooled_connections=possible_until_idle_timeout",
                    authority_host, backend_addr, previous_addrs, current_addrs, generation
                );
            }
            // Phase 4: rotation failure is no longer silent. The resolution update
            // above still stands, but surface that pooled clients were not rotated.
            if let Some(error) = client_rotation.failure() {
                warn!(
                    "backend client rotation failed for '{}' (backend '{}') after DNS refresh: {}; stale pooled connections persist until idle timeout",
                    authority_host, backend_addr, error
                );
            }
        }
        BackendDnsRefreshApplication::Unchanged {
            backend_addr,
            authority_host,
            current_addrs,
            generation,
            ..
        } => {
            debug!(
                "backend DNS refresh unchanged for '{}' (backend '{}') addrs={:?} generation={}",
                authority_host, backend_addr, current_addrs, generation
            );
        }
        BackendDnsRefreshApplication::EmptyAnswerRetained {
            backend_addr,
            authority_host,
            retained_addrs,
        } => {
            warn!(
                "backend DNS refresh returned no addresses for '{}' (backend '{}') [{}]; retaining {:?}; traffic continues on existing resolution (no manual action required unless persistent)",
                authority_host,
                backend_addr,
                BackendRefreshClassification::Rejected.slug(),
                retained_addrs
            );
        }
        BackendDnsRefreshApplication::LookupFailed {
            backend_addr,
            authority_host,
            retained_addrs,
            error,
        } => {
            warn!(
                "backend DNS refresh failed for '{}' (backend '{}') [{}]: {}; retaining {:?}; traffic continues on existing resolution (fix DNS/upstream if persistent)",
                authority_host,
                backend_addr,
                BackendRefreshClassification::FailedActivePreserved.slug(),
                error,
                retained_addrs
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr, time::Duration};

    use spooky_config::{
        config::{Backend, Config, HealthCheck, Listen, LoadBalancing, RouteMatch, Tls, Upstream},
        runtime::{RuntimeBackendTransportKind, RuntimeConfig},
    };
    use spooky_transport::{SharedDnsResolver, UpstreamTransportPool};

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
                route: RouteMatch::default(),
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
            spooky_config::runtime::RuntimeBackendConnectionPolicy {
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
        use super::*;

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
                Some(spooky_lb::health::HealthFailureReason::Transport),
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
        use super::*;

        #[test]
        fn active_health_check_evaluation_tracks_backoff_and_transition() {
            let pool = test_active_health_upstream_pool();

            let failure = evaluate_active_health_check(
                BackendIdentity::new("127.0.0.1:8080"),
                BackendHealthObservationOutcome::Failure,
                Some(spooky_lb::health::HealthFailureReason::Transport),
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
                Some(spooky_lb::health::HealthFailureReason::Transport),
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
                reason: Some(spooky_lb::health::HealthFailureReason::Timeout),
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
                reason: Some(spooky_lb::health::HealthFailureReason::Transport),
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
                reason: Some(spooky_lb::health::HealthFailureReason::Transport),
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
        use super::*;

        #[test]
        fn request_feedback_applier_marks_backend_unhealthy_after_failure_threshold() {
            let pool = test_upstream_pool();
            let feedback = BackendRequestFeedback::failure(
                BackendIdentity::new("127.0.0.1:8080"),
                Duration::from_millis(10),
                Some(503),
                Some(spooky_lb::health::HealthFailureReason::HttpStatus5xx),
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
                Some(spooky_lb::health::HealthFailureReason::Timeout),
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
                Some(spooky_lb::health::HealthFailureReason::Transport),
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
                Some(spooky_lb::health::HealthFailureReason::Timeout),
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
                    Some(spooky_lb::health::HealthFailureReason::HttpStatus5xx),
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
                    Some(spooky_lb::health::HealthFailureReason::HttpStatus5xx),
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
                    Some(spooky_lb::health::HealthFailureReason::HttpStatus5xx),
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
                    Some(spooky_lb::health::HealthFailureReason::HttpStatus5xx),
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
                Some(spooky_lb::health::HealthFailureReason::Transport),
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
